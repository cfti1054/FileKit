//! 트레이 프로세스의 파이프 서버 (tokio named pipe).
//!
//! - 첫 인스턴스를 `first_pipe_instance(true)`로 만들어 단일 인스턴스를 판정한다. 실패하면 이미 실행 중.
//! - `reject_remote_clients(true)` (`PIPE_REJECT_REMOTE_CLIENTS`).
//! - TODO: 현재 사용자만 접근하는 보안 설명자(SDDL `D:P(A;;GA;;;<사용자 SID>)`)를
//!   `ServerOptions::create_with_security_attributes_raw`로 건다. 지금은 기본 DACL
//!   (LocalSystem·Administrators·소유자 모든 권한, Everyone 읽기)이다.
//! - 연결마다: 줄 읽기 작업(1 MiB 상한) + 처리 루프(`handle_line`, `subscribe`면 작업 이벤트 전달).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use convtool_core::pipe::{encode_line, ServerMessage, MAX_LINE_BYTES, PIPE_NAME};
use convtool_core::{
    query_formats, BatchTicket, CoreError, FormatsReport, JobEvent, JobId, JobRequest,
};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, WriteHalf};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{broadcast, mpsc};

use super::{handle_line, ConnState, PipeBackend, MENU_COALESCE};
use crate::events::OpenedBy;
use crate::state::{lock, AppState};

const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_PIPE_BUSY: i32 = 231;

/// 첫 파이프 인스턴스를 만든다. tokio 런타임 안에서 불러야 한다.
pub fn bind_first() -> io::Result<NamedPipeServer> {
    ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(PIPE_NAME)
}

/// `bind_first` 오류가 "이미 다른 트레이가 실행 중"인지
pub fn is_already_running(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(ERROR_ACCESS_DENIED) | Some(ERROR_PIPE_BUSY)
    )
}

fn next_instance() -> io::Result<NamedPipeServer> {
    ServerOptions::new()
        .reject_remote_clients(true)
        .create(PIPE_NAME)
}

/// 연결을 계속 받는다. 앱이 끝날 때까지 돈다.
pub async fn serve(first: NamedPipeServer, app: AppHandle) {
    let mut server = first;
    loop {
        if let Err(e) = server.connect().await {
            tracing::warn!("파이프 연결 받기 실패: {e}");
        } else {
            let connected = server;
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = handle_conn(connected, app2).await {
                    tracing::debug!("파이프 연결 종료: {e}");
                }
            });
        }
        // 다음 클라이언트용 인스턴스. 실패하면 잠시 뒤 다시 만든다.
        server = loop {
            match next_instance() {
                Ok(s) => break s,
                Err(e) => {
                    tracing::error!("파이프 인스턴스 만들기 실패: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        };
    }
}

enum Incoming {
    Line(String),
    Eof,
    Bad(CoreError),
}

async fn read_lines<R: tokio::io::AsyncRead + Unpin>(rd: R, tx: mpsc::Sender<Incoming>) {
    let mut reader = BufReader::new(rd);
    loop {
        let mut buf = Vec::new();
        let limit = (MAX_LINE_BYTES + 1) as u64;
        let r = (&mut reader).take(limit).read_until(b'\n', &mut buf).await;
        let msg = match r {
            Ok(0) => Incoming::Eof,
            Ok(_) if buf.len() > MAX_LINE_BYTES && buf.last() != Some(&b'\n') => {
                Incoming::Bad(CoreError::InvalidInput(format!(
                    "파이프 메시지가 너무 깁니다 (최대 {MAX_LINE_BYTES} 바이트)"
                )))
            }
            Ok(_) => match String::from_utf8(buf) {
                Ok(s) => Incoming::Line(s),
                Err(_) => Incoming::Bad(CoreError::InvalidInput(
                    "파이프 메시지가 UTF-8이 아닙니다".into(),
                )),
            },
            Err(e) => Incoming::Bad(CoreError::Io {
                path: PathBuf::from(PIPE_NAME),
                source: e,
            }),
        };
        let stop = !matches!(msg, Incoming::Line(_));
        if tx.send(msg).await.is_err() || stop {
            break;
        }
    }
}

async fn write_msg(wr: &mut WriteHalf<NamedPipeServer>, msg: &ServerMessage) -> io::Result<()> {
    let line = encode_line(msg).map_err(|e| io::Error::other(e.to_string()))?;
    wr.write_all(line.as_bytes()).await?;
    wr.flush().await
}

async fn recv_event(
    rx: &mut Option<broadcast::Receiver<JobEvent>>,
) -> Result<JobEvent, broadcast::error::RecvError> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

async fn handle_conn(pipe: NamedPipeServer, app: AppHandle) -> io::Result<()> {
    let (rd, mut wr) = tokio::io::split(pipe);
    let (line_tx, mut line_rx) = mpsc::channel::<Incoming>(16);
    let reader = tauri::async_runtime::spawn(read_lines(rd, line_tx));

    let backend = TrayBackend { app: app.clone() };
    let mut conn = ConnState::default();
    let mut events: Option<broadcast::Receiver<JobEvent>> = None;

    let result = loop {
        tokio::select! {
            incoming = line_rx.recv() => {
                let line = match incoming {
                    None | Some(Incoming::Eof) => break Ok(()),
                    Some(Incoming::Bad(e)) => {
                        let msg = ServerMessage::Error { request_id: None, error: e.info() };
                        let _ = write_msg(&mut wr, &msg).await;
                        break Ok(());
                    }
                    Some(Incoming::Line(l)) => l,
                };
                if matches!(conn, ConnState::Ready { .. }) {
                    // hello 뒤의 요청은 엔진 확인(probe)이 끝난 코어가 필요하다.
                    app.state::<AppState>().core().await;
                }
                let reply = handle_line(&mut conn, &line, &backend);
                for m in &reply.messages {
                    write_msg(&mut wr, m).await?;
                }
                if reply.subscribe && events.is_none() {
                    events = Some(app.state::<AppState>().events.subscribe());
                }
                if reply.close {
                    break Ok(());
                }
            }
            ev = recv_event(&mut events) => match ev {
                Ok(event) => write_msg(&mut wr, &ServerMessage::Event { event }).await?,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("파이프 구독자가 이벤트 {n}개를 놓쳤습니다");
                }
                Err(broadcast::error::RecvError::Closed) => events = None,
            }
        }
    };
    reader.abort();
    result
}

/// 파이프 요청을 앱 상태로 처리한다
struct TrayBackend {
    app: AppHandle,
}

impl TrayBackend {
    fn core(&self) -> Result<std::sync::Arc<crate::state::Core>, CoreError> {
        self.app
            .state::<AppState>()
            .core_now()
            .ok_or_else(|| CoreError::Internal("엔진을 확인하는 중입니다".into()))
    }
}

impl PipeBackend for TrayBackend {
    fn query_formats(
        &self,
        files: &[PathBuf],
        primary: Option<&Path>,
    ) -> Result<FormatsReport, CoreError> {
        query_formats(files, primary, &self.core()?.engines)
    }

    fn submit(&self, request: JobRequest) -> Result<BatchTicket, CoreError> {
        self.core()?.queue.submit(request)
    }

    fn cancel(&self, job_id: JobId) -> bool {
        self.core().is_ok_and(|c| c.queue.cancel(job_id))
    }

    fn show_menu(&self, files: Vec<PathBuf>, primary: Option<PathBuf>) -> Result<(), CoreError> {
        if files.is_empty() {
            return Err(CoreError::InvalidInput("파일이 없습니다".into()));
        }
        let started = lock(&self.app.state::<AppState>().menu_batcher).push(files, primary);
        if started {
            let app = self.app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(MENU_COALESCE).await;
                let pending = lock(&app.state::<AppState>().menu_batcher).flush();
                if let Some(p) = pending {
                    // 창 조작은 tauri가 메인 스레드로 넘긴다. 블로킹 호출이 섞여 있어 blocking 스레드에서 부른다.
                    let app2 = app.clone();
                    let r = tauri::async_runtime::spawn_blocking(move || {
                        crate::overlay::open_menu(
                            &app2,
                            p.files,
                            p.primary,
                            None,
                            OpenedBy::ContextMenu,
                            None,
                        )
                    })
                    .await;
                    match r {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => app.state::<AppState>().notifier.error(&e),
                        Err(e) => tracing::error!("원형 메뉴 열기 작업 실패: {e}"),
                    }
                }
            });
        }
        Ok(())
    }

    fn show_main_window(&self, files: Vec<PathBuf>) -> Result<(), CoreError> {
        if !files.is_empty() {
            // TODO(인터페이스): 이미 떠 있는 메인 창에 파일을 넘길 이벤트가 없다.
            // get_launch_files는 페이지가 처음 뜰 때만 읽으므로, 지금은 다음 로드 때 쓰이도록 쌓아 둔다.
            lock(&self.app.state::<AppState>().launch_files).extend(files);
        }
        crate::tray::show_main(&self.app);
        Ok(())
    }
}
