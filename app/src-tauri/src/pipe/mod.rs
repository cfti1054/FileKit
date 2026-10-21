//! 이름 있는 파이프 `\\.\pipe\convtool` (설계 계획 경계 (2)).
//!
//! 이 파일은 순수 로직이다(tokio·Win32 없음): 연결별 프로토콜 상태 기계(`handle_line`)와
//! 우클릭 `show_menu` 요청 묶기(`MenuBatcher`). 메시지 타입과 줄 인코딩은 `convtool_core::pipe`가 가진다.
//!
//! - `server`(tauri-app): 트레이 프로세스의 tokio 서버. `first_pipe_instance`로 단일 인스턴스 판정.
//! - `client`(tauri-app): 두 번째 실행(창 모드, `convert --ask`)이 요청 하나를 넘기는 클라이언트.

use std::path::{Path, PathBuf};
use std::time::Duration;

use convtool_core::pipe::{
    decode_client, ClientKind, ClientMessage, ServerMessage, PROTOCOL_VERSION,
};
use convtool_core::{BatchTicket, CoreError, FormatsReport, JobId, JobRequest};

#[cfg(feature = "tauri-app")]
pub mod client;
pub mod security;
#[cfg(feature = "tauri-app")]
pub mod server;

/// 셸 동사는 선택 파일마다 프로세스를 하나씩 띄운다(가정). 이 시간 안에 온 `show_menu`는 한 세션으로 합친다.
pub const MENU_COALESCE: Duration = Duration::from_millis(300);

/// 파이프 요청을 실제로 처리하는 쪽 (트레이 앱 상태). 테스트에서는 가짜를 끼운다.
pub trait PipeBackend {
    fn query_formats(
        &self,
        files: &[PathBuf],
        primary: Option<&Path>,
    ) -> Result<FormatsReport, CoreError>;
    fn submit(&self, request: JobRequest) -> Result<BatchTicket, CoreError>;
    fn cancel(&self, job_id: JobId) -> bool;
    fn show_menu(&self, files: Vec<PathBuf>, primary: Option<PathBuf>) -> Result<(), CoreError>;
    fn show_main_window(&self, files: Vec<PathBuf>) -> Result<(), CoreError>;
}

/// 연결 하나의 상태
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnState {
    #[default]
    AwaitHello,
    Ready {
        client: ClientKind,
    },
}

/// 줄 하나를 처리한 결과
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reply {
    pub messages: Vec<ServerMessage>,
    /// 보낸 뒤 연결을 끊는다
    pub close: bool,
    /// 이후 모든 `event`를 이 연결에 보낸다
    pub subscribe: bool,
}

impl Reply {
    fn one(msg: ServerMessage) -> Self {
        Self {
            messages: vec![msg],
            ..Self::default()
        }
    }

    fn error(request_id: Option<u64>, err: &CoreError, close: bool) -> Self {
        Self {
            messages: vec![ServerMessage::Error {
                request_id,
                error: err.info(),
            }],
            close,
            subscribe: false,
        }
    }
}

fn request_id_of(msg: &ClientMessage) -> Option<u64> {
    match msg {
        ClientMessage::Hello { .. } => None,
        ClientMessage::QueryFormats { request_id, .. }
        | ClientMessage::Submit { request_id, .. }
        | ClientMessage::Cancel { request_id, .. }
        | ClientMessage::Subscribe { request_id }
        | ClientMessage::ShowMenu { request_id, .. }
        | ClientMessage::ShowMainWindow { request_id, .. } => Some(*request_id),
    }
}

/// 받은 줄 하나를 처리한다.
///
/// 규칙: 연결하자마자 `hello`가 와야 한다. 다른 메시지가 먼저 오거나 protocol이 다르면 `error`를 보내고 끊는다.
pub fn handle_line(state: &mut ConnState, line: &str, backend: &dyn PipeBackend) -> Reply {
    let msg = match decode_client(line) {
        Ok(m) => m,
        Err(e) => {
            // hello 전의 잘못된 줄은 끊는다. 그 뒤에는 오류만 알리고 연결을 유지한다.
            let close = *state == ConnState::AwaitHello;
            return Reply::error(None, &e, close);
        }
    };

    match (*state, msg) {
        (
            ConnState::AwaitHello,
            ClientMessage::Hello {
                protocol,
                client,
                pid,
            },
        ) => {
            if protocol != PROTOCOL_VERSION {
                let e = CoreError::InvalidInput(format!(
                    "파이프 프로토콜 버전이 다릅니다 (클라이언트 {protocol}, 서버 {PROTOCOL_VERSION})"
                ));
                return Reply::error(None, &e, true);
            }
            tracing::debug!(?client, pid, "파이프 클라이언트 hello");
            *state = ConnState::Ready { client };
            Reply::one(ServerMessage::HelloAck {
                protocol: PROTOCOL_VERSION,
                server_version: convtool_core::VERSION.to_string(),
            })
        }
        (ConnState::AwaitHello, other) => {
            let e = CoreError::InvalidInput("연결 후 첫 메시지는 hello여야 합니다".into());
            Reply::error(request_id_of(&other), &e, true)
        }
        (ConnState::Ready { .. }, ClientMessage::Hello { .. }) => {
            let e = CoreError::InvalidInput("hello를 이미 받았습니다".into());
            Reply::error(None, &e, false)
        }
        (
            ConnState::Ready { .. },
            ClientMessage::QueryFormats {
                request_id,
                files,
                primary,
            },
        ) => match backend.query_formats(&files, primary.as_deref()) {
            Ok(report) => Reply::one(ServerMessage::Formats { request_id, report }),
            Err(e) => Reply::error(Some(request_id), &e, false),
        },
        (
            ConnState::Ready { .. },
            ClientMessage::Submit {
                request_id,
                request,
            },
        ) => match backend.submit(request) {
            Ok(ticket) => Reply::one(ServerMessage::Submitted { request_id, ticket }),
            Err(e) => Reply::error(Some(request_id), &e, false),
        },
        (ConnState::Ready { .. }, ClientMessage::Cancel { request_id, job_id }) => {
            Reply::one(ServerMessage::Ack {
                request_id,
                ok: backend.cancel(job_id),
            })
        }
        (ConnState::Ready { .. }, ClientMessage::Subscribe { request_id }) => Reply {
            messages: vec![ServerMessage::Ack {
                request_id,
                ok: true,
            }],
            close: false,
            subscribe: true,
        },
        (
            ConnState::Ready { .. },
            ClientMessage::ShowMenu {
                request_id,
                files,
                primary,
            },
        ) => match backend.show_menu(files, primary) {
            Ok(()) => Reply::one(ServerMessage::Ack {
                request_id,
                ok: true,
            }),
            Err(e) => Reply::error(Some(request_id), &e, false),
        },
        (ConnState::Ready { .. }, ClientMessage::ShowMainWindow { request_id, files }) => {
            match backend.show_main_window(files) {
                Ok(()) => Reply::one(ServerMessage::Ack {
                    request_id,
                    ok: true,
                }),
                Err(e) => Reply::error(Some(request_id), &e, false),
            }
        }
    }
}

/// 묶인 `show_menu` 요청
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PendingMenu {
    pub files: Vec<PathBuf>,
    pub primary: Option<PathBuf>,
}

/// `MENU_COALESCE` 안에 들어온 `show_menu`를 한 세션으로 합친다. 시간은 호출하는 쪽(타이머)이 잰다.
#[derive(Debug, Default)]
pub struct MenuBatcher {
    pending: Option<PendingMenu>,
}

fn path_key(p: &Path) -> String {
    p.as_os_str().to_string_lossy().to_lowercase()
}

impl MenuBatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// 요청을 더한다. 새 묶음을 시작했으면 true (호출하는 쪽이 `MENU_COALESCE` 뒤 `flush`를 예약한다).
    /// 같은 파일(대소문자 무시)은 한 번만 넣는다. primary는 처음 들어온 값을 쓴다.
    pub fn push(&mut self, files: Vec<PathBuf>, primary: Option<PathBuf>) -> bool {
        let started = self.pending.is_none();
        let p = self.pending.get_or_insert_with(PendingMenu::default);
        for f in files {
            let k = path_key(&f);
            if !p.files.iter().any(|x| path_key(x) == k) {
                p.files.push(f);
            }
        }
        if p.primary.is_none() {
            p.primary = primary;
        }
        started
    }

    pub fn flush(&mut self) -> Option<PendingMenu> {
        self.pending.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use convtool_core::pipe::{decode_server, encode_line};
    use convtool_core::{RadialLayout, RadialSlot};
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<String>>,
        fail_menu: bool,
    }

    impl PipeBackend for Fake {
        fn query_formats(
            &self,
            files: &[PathBuf],
            _primary: Option<&Path>,
        ) -> Result<FormatsReport, CoreError> {
            self.calls
                .borrow_mut()
                .push(format!("query {}", files.len()));
            if files.is_empty() {
                return Err(CoreError::UnsupportedFormat {
                    input: String::new(),
                    to: None,
                });
            }
            Ok(FormatsReport {
                files: vec![],
                skipped: vec![],
                options: vec![],
                layout: RadialLayout {
                    slot_count: 4,
                    slots: vec![RadialSlot::Empty; 4],
                    more: vec![],
                },
            })
        }
        fn submit(&self, request: JobRequest) -> Result<BatchTicket, CoreError> {
            self.calls
                .borrow_mut()
                .push(format!("submit {}", request.to));
            Ok(BatchTicket {
                batch_id: 1,
                jobs: vec![],
                rejected: vec![],
            })
        }
        fn cancel(&self, job_id: JobId) -> bool {
            self.calls.borrow_mut().push(format!("cancel {job_id}"));
            job_id == 7
        }
        fn show_menu(&self, files: Vec<PathBuf>, _p: Option<PathBuf>) -> Result<(), CoreError> {
            self.calls
                .borrow_mut()
                .push(format!("menu {}", files.len()));
            if self.fail_menu {
                Err(CoreError::Internal("x".into()))
            } else {
                Ok(())
            }
        }
        fn show_main_window(&self, files: Vec<PathBuf>) -> Result<(), CoreError> {
            self.calls
                .borrow_mut()
                .push(format!("main {}", files.len()));
            Ok(())
        }
    }

    const HELLO: &str = r#"{"type":"hello","protocol":1,"client":"shell","pid":4120}"#;

    fn ready(fake: &Fake) -> ConnState {
        let mut st = ConnState::default();
        let r = handle_line(&mut st, HELLO, fake);
        assert!(!r.close);
        st
    }

    #[test]
    fn hello_ack_and_state() {
        let fake = Fake::default();
        let mut st = ConnState::default();
        let r = handle_line(&mut st, &format!("{HELLO}\n"), &fake);
        assert_eq!(
            r.messages,
            vec![ServerMessage::HelloAck {
                protocol: 1,
                server_version: convtool_core::VERSION.into()
            }]
        );
        assert_eq!(
            st,
            ConnState::Ready {
                client: ClientKind::Shell
            }
        );
        // JSON 모양 (계획 (2) 예시)
        let line = encode_line(&r.messages[0]).unwrap();
        assert!(line.starts_with(r#"{"type":"hello_ack","protocol":1,"server_version":""#));
        assert!(matches!(
            decode_server(&line).unwrap(),
            ServerMessage::HelloAck { .. }
        ));
    }

    #[test]
    fn wrong_protocol_closes() {
        let fake = Fake::default();
        let mut st = ConnState::default();
        let r = handle_line(
            &mut st,
            r#"{"type":"hello","protocol":2,"client":"cli","pid":1}"#,
            &fake,
        );
        assert!(r.close);
        assert!(matches!(
            &r.messages[0],
            ServerMessage::Error { request_id: None, error } if error.code == convtool_core::ErrorCode::InvalidInput
        ));
        assert_eq!(st, ConnState::AwaitHello);
    }

    #[test]
    fn message_before_hello_closes_with_request_id() {
        let fake = Fake::default();
        let mut st = ConnState::default();
        let r = handle_line(&mut st, r#"{"type":"subscribe","request_id":4}"#, &fake);
        assert!(r.close);
        assert!(matches!(
            &r.messages[0],
            ServerMessage::Error {
                request_id: Some(4),
                ..
            }
        ));
        assert!(fake.calls.borrow().is_empty());
    }

    #[test]
    fn garbage_before_hello_closes_after_hello_keeps() {
        let fake = Fake::default();
        let mut st = ConnState::default();
        assert!(handle_line(&mut st, "not json", &fake).close);
        let mut st = ready(&fake);
        let r = handle_line(&mut st, "not json", &fake);
        assert!(!r.close);
        assert!(matches!(
            &r.messages[0],
            ServerMessage::Error {
                request_id: None,
                ..
            }
        ));
        let r = handle_line(&mut st, HELLO, &fake);
        assert!(!r.close);
        assert!(matches!(&r.messages[0], ServerMessage::Error { .. }));
    }

    #[test]
    fn query_submit_cancel_subscribe() {
        let fake = Fake::default();
        let mut st = ready(&fake);
        let r = handle_line(
            &mut st,
            r#"{"type":"query_formats","request_id":1,"files":["C:\\영상\\발표 녹화.mov"],"primary":"C:\\영상\\발표 녹화.mov"}"#,
            &fake,
        );
        assert!(matches!(
            &r.messages[0],
            ServerMessage::Formats { request_id: 1, .. }
        ));
        let r = handle_line(
            &mut st,
            r#"{"type":"query_formats","request_id":9,"files":[],"primary":null}"#,
            &fake,
        );
        assert!(matches!(
            &r.messages[0],
            ServerMessage::Error { request_id: Some(9), error } if error.code == convtool_core::ErrorCode::UnsupportedFormat
        ));
        let r = handle_line(
            &mut st,
            r#"{"type":"submit","request_id":2,"request":{"files":["C:\\영상\\발표 녹화.mov"],"to":"mp4","preset":"web","out_dir":null,"overwrite":false}}"#,
            &fake,
        );
        assert!(matches!(
            &r.messages[0],
            ServerMessage::Submitted { request_id: 2, .. }
        ));
        let r = handle_line(
            &mut st,
            r#"{"type":"cancel","request_id":3,"job_id":7}"#,
            &fake,
        );
        assert_eq!(
            r.messages,
            vec![ServerMessage::Ack {
                request_id: 3,
                ok: true
            }]
        );
        let r = handle_line(
            &mut st,
            r#"{"type":"cancel","request_id":5,"job_id":8}"#,
            &fake,
        );
        assert_eq!(
            r.messages,
            vec![ServerMessage::Ack {
                request_id: 5,
                ok: false
            }]
        );
        let r = handle_line(&mut st, r#"{"type":"subscribe","request_id":4}"#, &fake);
        assert!(r.subscribe);
        assert!(!r.close);
        assert_eq!(
            *fake.calls.borrow(),
            vec!["query 1", "query 0", "submit mp4", "cancel 7", "cancel 8"]
        );
    }

    #[test]
    fn show_menu_and_main_window() {
        let fake = Fake::default();
        let mut st = ready(&fake);
        let r = handle_line(
            &mut st,
            r#"{"type":"show_menu","request_id":5,"files":["C:\\영상\\발표 녹화.mov"],"primary":"C:\\영상\\발표 녹화.mov"}"#,
            &fake,
        );
        assert_eq!(
            r.messages,
            vec![ServerMessage::Ack {
                request_id: 5,
                ok: true
            }]
        );
        let r = handle_line(
            &mut st,
            r#"{"type":"show_main_window","request_id":6,"files":[]}"#,
            &fake,
        );
        assert_eq!(
            r.messages,
            vec![ServerMessage::Ack {
                request_id: 6,
                ok: true
            }]
        );
        let failing = Fake {
            fail_menu: true,
            ..Fake::default()
        };
        let mut st = ready(&failing);
        let r = handle_line(
            &mut st,
            r#"{"type":"show_menu","request_id":8,"files":[],"primary":null}"#,
            &failing,
        );
        assert!(matches!(
            &r.messages[0],
            ServerMessage::Error {
                request_id: Some(8),
                ..
            }
        ));
    }

    #[test]
    fn batcher_merges_and_dedupes() {
        let mut b = MenuBatcher::new();
        assert!(b.flush().is_none());
        assert!(b.push(
            vec![PathBuf::from(r"C:\a.mov")],
            Some(PathBuf::from(r"C:\a.mov"))
        ));
        assert!(!b.push(
            vec![PathBuf::from(r"C:\b.mov")],
            Some(PathBuf::from(r"C:\b.mov"))
        ));
        assert!(!b.push(vec![PathBuf::from(r"c:\A.MOV")], None));
        let p = b.flush().unwrap();
        assert_eq!(
            p.files,
            vec![PathBuf::from(r"C:\a.mov"), PathBuf::from(r"C:\b.mov")]
        );
        assert_eq!(p.primary, Some(PathBuf::from(r"C:\a.mov")));
        assert!(b.flush().is_none());
        // 비운 뒤에는 새 묶음
        assert!(b.push(vec![PathBuf::from(r"C:\c.mov")], None));
        let p = b.flush().unwrap();
        assert_eq!(p.primary, None);
    }

    #[test]
    fn batcher_takes_first_non_null_primary() {
        let mut b = MenuBatcher::new();
        b.push(vec![PathBuf::from("a.mov")], None);
        b.push(vec![PathBuf::from("b.mov")], Some(PathBuf::from("b.mov")));
        assert_eq!(b.flush().unwrap().primary, Some(PathBuf::from("b.mov")));
    }
}
