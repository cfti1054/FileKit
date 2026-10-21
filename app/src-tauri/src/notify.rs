//! 알림 (F-05): 로그 + Windows 토스트.
//!
//! - 발신자 이름: AUMID `com.filekit.convtool`. 앱 시작 시 `set_process_aumid`
//!   (`SetCurrentProcessExplicitAppUserModelID`), installer/hooks.nsh가 `HKCU\Software\Classes\AppUserModelId`에
//!   `DisplayName=FileKit`을 등록한다. Tauri NSIS 바로가기에도 같은 AUMID(identifier)가 붙는다.
//! - 배치 하나 = 토스트 하나(태그 `batch-<id>`): 시작하면 진행률 막대, 진행·완료 개수는 같은 알림을 갱신,
//!   배치가 끝나면 같은 태그로 완료(“폴더에서 보기”)·실패(원인 한 줄) 알림으로 바꾼다.
//! - 판정(`ToastPlanner`)과 XML 만들기는 순수 코드다. WinRT 호출은 전용 스레드(MTA)에서 하고,
//!   실패해도 로그만 남긴다(변환은 계속).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use convtool_core::{BatchId, ErrorInfo, JobEvent, JobId};

/// 토스트 발신자 AUMID (tauri.conf.json identifier, installer/hooks.nsh와 같다)
pub const AUMID: &str = "com.filekit.convtool";
/// 토스트 그룹 (태그와 함께 갱신·삭제 키)
pub const TOAST_GROUP: &str = "convtool";
/// 진행률 알림 갱신 최소 간격
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

/// 알림 받는 쪽. 기본 구현은 아무것도 하지 않는다.
pub trait Notifier: Send + Sync {
    fn job_queued(&self, _batch_id: BatchId, _job_id: JobId, _input: &Path) {}
    fn job_started(&self, _batch_id: BatchId, _job_id: JobId) {}
    fn job_progress(&self, _job_id: JobId, _progress: Option<f64>) {}
    fn job_done(&self, _job_id: JobId, _input: &Path, _output: &Path) {}
    fn job_failed(&self, _job_id: JobId, _input: &Path, _error: &ErrorInfo) {}
    fn batch_done(&self, _batch_id: BatchId, _succeeded: u32, _failed: u32) {}
    /// 작업 큐에 들어가기 전 오류 (원형 메뉴에서 고른 형식을 넣지 못함, 지원 형식 없음 등)
    fn error(&self, _error: &ErrorInfo) {}
    /// 작업 이벤트 하나. 기본은 메서드별로 나눈다(`dispatch`).
    /// 앱 종료: 알림 스레드를 끝낸다.
    fn shutdown(&self) {}
    fn event(&self, ev: &JobEvent) {
        dispatch(self, ev);
    }
}

/// `JobEvent`를 알맞은 메서드로 나눈다.
pub fn dispatch<N: Notifier + ?Sized>(n: &N, ev: &JobEvent) {
    match ev {
        JobEvent::Queued {
            batch_id,
            job_id,
            input,
            ..
        } => n.job_queued(*batch_id, *job_id, input),
        JobEvent::Started {
            batch_id, job_id, ..
        } => n.job_started(*batch_id, *job_id),
        JobEvent::Progress {
            job_id, progress, ..
        } => n.job_progress(*job_id, *progress),
        JobEvent::Done {
            job_id,
            input,
            output,
            ..
        } => n.job_done(*job_id, input, output),
        JobEvent::Failed {
            job_id,
            input,
            error,
            ..
        } => n.job_failed(*job_id, input, error),
        JobEvent::Rejected { error, .. } => n.error(error),
        JobEvent::BatchDone {
            batch_id,
            succeeded,
            failed,
            ..
        } => n.batch_done(*batch_id, *succeeded, *failed),
        JobEvent::Canceled { .. } => {}
    }
}

/// 로그만 남기는 구현
#[derive(Debug, Default)]
pub struct LogNotifier;

impl Notifier for LogNotifier {
    fn job_started(&self, batch_id: BatchId, job_id: JobId) {
        tracing::info!(batch_id, job_id, "변환 시작");
    }
    fn job_done(&self, job_id: JobId, input: &Path, output: &Path) {
        tracing::info!(job_id, input = %input.display(), output = %output.display(), "변환 완료");
    }
    fn job_failed(&self, job_id: JobId, input: &Path, error: &ErrorInfo) {
        tracing::warn!(job_id, input = %input.display(), code = ?error.code, "변환 실패: {}", error.message);
    }
    fn batch_done(&self, batch_id: BatchId, succeeded: u32, failed: u32) {
        tracing::info!(batch_id, succeeded, failed, "배치 완료");
    }
    fn error(&self, error: &ErrorInfo) {
        tracing::warn!(code = ?error.code, "{}", error.message);
    }
}

// ---------------------------------------------------------------------------
// 토스트 판정 (순수)

#[derive(Debug, Clone, PartialEq)]
pub struct ToastProgress {
    /// 0.0~1.0
    pub value: f64,
    /// 막대 오른쪽 글자 ("1/3")
    pub value_text: String,
    /// 막대 아래 글자 ("변환 중")
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToastContent {
    pub tag: String,
    pub title: String,
    pub body: String,
    pub progress: Option<ToastProgress>,
    /// "폴더에서 보기" 버튼이 열 폴더
    pub open_folder: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToastAction {
    /// 새로 띄우거나 같은 태그의 알림을 바꾼다
    Show(ToastContent),
    /// 진행률만 갱신
    Update {
        tag: String,
        progress: ToastProgress,
    },
    /// 알림 지우기 (모두 취소된 배치)
    Remove { tag: String },
}

#[derive(Debug, Default)]
struct BatchToast {
    total: u32,
    finished: u32,
    running: HashMap<JobId, Option<f64>>,
    shown: bool,
    last_update: Option<Instant>,
    first_input: Option<PathBuf>,
    last_output: Option<PathBuf>,
    first_error: Option<String>,
}

impl BatchToast {
    fn progress(&self) -> ToastProgress {
        let total = self.total.max(1);
        let running: f64 = self.running.values().map(|p| p.unwrap_or(0.0)).sum();
        let value = ((f64::from(self.finished) + running) / f64::from(total)).clamp(0.0, 1.0);
        ToastProgress {
            value,
            value_text: format!("{}/{}", self.finished, self.total),
            status: "변환 중".into(),
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn batch_tag(batch_id: BatchId) -> String {
    format!("batch-{batch_id}")
}

/// 작업 이벤트 → 토스트 동작
#[derive(Debug, Default)]
pub struct ToastPlanner {
    batches: HashMap<BatchId, BatchToast>,
    errors: u64,
}

impl ToastPlanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_error(&mut self, error: &ErrorInfo) -> ToastAction {
        self.errors += 1;
        ToastAction::Show(ToastContent {
            tag: format!("error-{}", self.errors),
            title: "변환할 수 없음".into(),
            body: error.message.clone(),
            progress: None,
            open_folder: None,
        })
    }

    pub fn on_event(&mut self, ev: &JobEvent, now: Instant) -> Option<ToastAction> {
        match ev {
            JobEvent::Queued {
                batch_id, input, ..
            } => {
                let b = self.batches.entry(*batch_id).or_default();
                b.total += 1;
                b.first_input.get_or_insert_with(|| input.clone());
                None
            }
            JobEvent::Rejected {
                batch_id, error, ..
            } => {
                let b = self.batches.entry(*batch_id).or_default();
                b.first_error.get_or_insert_with(|| error.message.clone());
                None
            }
            JobEvent::Started {
                batch_id, job_id, ..
            } => {
                let b = self.batches.entry(*batch_id).or_default();
                b.running.insert(*job_id, None);
                if b.shown {
                    return None;
                }
                b.shown = true;
                b.last_update = Some(now);
                let body = match (&b.first_input, b.total) {
                    (Some(p), 0 | 1) => file_name(p),
                    (_, n) => format!("파일 {n}개"),
                };
                Some(ToastAction::Show(ToastContent {
                    tag: batch_tag(*batch_id),
                    title: "변환 중".into(),
                    body,
                    progress: Some(b.progress()),
                    open_folder: None,
                }))
            }
            JobEvent::Progress {
                batch_id,
                job_id,
                progress,
            } => {
                let b = self.batches.get_mut(batch_id)?;
                b.running.insert(*job_id, *progress);
                let due = b
                    .last_update
                    .is_none_or(|t| now.saturating_duration_since(t) >= PROGRESS_INTERVAL);
                if !b.shown || !due {
                    return None;
                }
                b.last_update = Some(now);
                Some(ToastAction::Update {
                    tag: batch_tag(*batch_id),
                    progress: b.progress(),
                })
            }
            JobEvent::Done {
                batch_id,
                job_id,
                output,
                ..
            } => {
                let b = self.batches.get_mut(batch_id)?;
                b.last_output = Some(output.clone());
                Self::finish_job(*batch_id, b, *job_id, now)
            }
            JobEvent::Failed {
                batch_id,
                job_id,
                error,
                ..
            } => {
                let b = self.batches.get_mut(batch_id)?;
                b.first_error.get_or_insert_with(|| error.message.clone());
                Self::finish_job(*batch_id, b, *job_id, now)
            }
            JobEvent::Canceled {
                batch_id, job_id, ..
            } => {
                let b = self.batches.get_mut(batch_id)?;
                Self::finish_job(*batch_id, b, *job_id, now)
            }
            JobEvent::BatchDone {
                batch_id,
                succeeded,
                failed,
                rejected,
                ..
            } => {
                let b = self.batches.remove(batch_id).unwrap_or_default();
                Self::final_toast(*batch_id, &b, *succeeded, failed + rejected)
            }
        }
    }

    fn finish_job(
        batch_id: BatchId,
        b: &mut BatchToast,
        job_id: JobId,
        now: Instant,
    ) -> Option<ToastAction> {
        b.running.remove(&job_id);
        b.finished += 1;
        if !b.shown {
            return None;
        }
        b.last_update = Some(now);
        Some(ToastAction::Update {
            tag: batch_tag(batch_id),
            progress: b.progress(),
        })
    }

    fn final_toast(
        batch_id: BatchId,
        b: &BatchToast,
        succeeded: u32,
        failed: u32,
    ) -> Option<ToastAction> {
        let tag = batch_tag(batch_id);
        let open_folder = b
            .last_output
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        if failed == 0 && succeeded > 0 {
            let body = match (&b.last_output, succeeded) {
                (Some(out), 1) => file_name(out),
                _ => format!("파일 {succeeded}개를 변환했습니다"),
            };
            return Some(ToastAction::Show(ToastContent {
                tag,
                title: "변환 완료".into(),
                body,
                progress: None,
                open_folder,
            }));
        }
        if failed > 0 {
            let reason = b.first_error.clone().unwrap_or_default();
            let (title, body) = if succeeded == 0 {
                ("변환 실패", reason)
            } else {
                (
                    "일부 변환 실패",
                    format!("{succeeded}개 완료, {failed}개 실패: {reason}"),
                )
            };
            return Some(ToastAction::Show(ToastContent {
                tag,
                title: title.into(),
                body,
                progress: None,
                open_folder: if succeeded > 0 { open_folder } else { None },
            }));
        }
        // 모두 취소: 진행 알림만 지운다
        b.shown.then_some(ToastAction::Remove { tag })
    }
}

/// XML 특수 문자 이스케이프 (토스트 XML)
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0에 넣을 수 없는 제어 문자는 버린다
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// 폴더 경로 → `file:` URI (토스트 protocol 버튼). `C:\a b\` → `file:///C:/a%20b/`, UNC → `file://server/share/`.
pub fn folder_file_uri(folder: &Path) -> String {
    let s = folder.to_string_lossy().replace('\\', "/");
    let (prefix, rest) = match s.strip_prefix("//") {
        Some(unc) => ("file://", unc.to_string()),
        None => ("file:///", s),
    };
    let mut out = String::from(prefix);
    for b in rest.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    if !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// 토스트 XML. 진행률은 데이터 바인딩(`{progressValue}` 등)이라 같은 알림을 갱신할 수 있다.
pub fn toast_xml(c: &ToastContent) -> String {
    let mut xml = String::from(r#"<toast><visual><binding template="ToastGeneric">"#);
    xml.push_str(&format!(
        "<text>{}</text><text>{}</text>",
        xml_escape(&c.title),
        xml_escape(&c.body)
    ));
    if c.progress.is_some() {
        xml.push_str(r#"<progress value="{progressValue}" valueStringOverride="{progressValueString}" status="{progressStatus}"/>"#);
    }
    xml.push_str("</binding></visual>");
    if let Some(folder) = &c.open_folder {
        xml.push_str(&format!(
            r#"<actions><action content="폴더에서 보기" activationType="protocol" arguments="{}"/></actions>"#,
            xml_escape(&folder_file_uri(folder))
        ));
    }
    xml.push_str("</toast>");
    xml
}

/// 진행률 바인딩 값 (키, 값)
pub fn progress_values(p: &ToastProgress) -> [(&'static str, String); 3] {
    [
        ("progressValue", format!("{:.3}", p.value)),
        ("progressValueString", p.value_text.clone()),
        ("progressStatus", p.status.clone()),
    ]
}

#[cfg(windows)]
pub use win::{set_process_aumid, ToastNotifier};

#[cfg(windows)]
mod win {

    use std::sync::mpsc::{self, Sender};
    use std::sync::Mutex;
    use std::time::Instant;

    use convtool_core::{ErrorInfo, JobEvent};
    use windows::core::{HSTRING, PCWSTR};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::UI::Notifications::{
        NotificationData, ToastNotification, ToastNotificationManager, ToastNotifier as WinNotifier,
    };

    use super::{
        dispatch, progress_values, toast_xml, LogNotifier, Notifier, ToastAction, ToastPlanner,
        ToastProgress, AUMID, TOAST_GROUP,
    };

    /// 프로세스 AUMID. 창을 만들기 전에 부른다(작업 표시줄 묶음·토스트 발신자).
    pub fn set_process_aumid() {
        let id = HSTRING::from(AUMID);
        // SAFETY: NUL 종료 문자열을 넘긴다.
        if let Err(e) = unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(id.as_ptr())) } {
            tracing::warn!("AUMID 설정 실패: {e}");
        }
    }

    enum Msg {
        Event(JobEvent),
        Error(ErrorInfo),
    }

    /// 로그 + 토스트. WinRT 호출은 전용 스레드에서 한다.
    pub struct ToastNotifier {
        tx: Mutex<Option<Sender<Msg>>>,
    }

    impl ToastNotifier {
        /// 토스트 스레드를 시작한다. 실패하면 None(로그만 남기는 구현을 쓴다).
        pub fn start() -> Option<Self> {
            let (tx, rx) = mpsc::channel::<Msg>();
            let spawned = std::thread::Builder::new()
                .name("convtool-toast".into())
                .spawn(move || {
                    // SAFETY: 이 스레드를 MTA로 초기화한다(WinRT 호출용).
                    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                    let notifier =
                        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID));
                    let notifier = match notifier {
                        Ok(n) => Some(n),
                        Err(e) => {
                            tracing::warn!("토스트 알림을 쓸 수 없습니다(로그만 남김): {e}");
                            None
                        }
                    };
                    let mut planner = ToastPlanner::new();
                    let mut seq: u32 = 0;
                    while let Ok(msg) = rx.recv() {
                        let action = match msg {
                            Msg::Event(ev) => planner.on_event(&ev, Instant::now()),
                            Msg::Error(e) => Some(planner.on_error(&e)),
                        };
                        if let (Some(n), Some(a)) = (&notifier, action) {
                            seq = seq.wrapping_add(1);
                            if let Err(e) = run(n, &a, seq) {
                                tracing::warn!("토스트 알림 실패: {e}");
                            }
                        }
                    }
                    drop(notifier);
                    if hr.is_ok() {
                        // SAFETY: 위 CoInitializeEx와 짝
                        unsafe { CoUninitialize() };
                    }
                });
            match spawned {
                Ok(_) => Some(Self {
                    tx: Mutex::new(Some(tx)),
                }),
                Err(e) => {
                    tracing::warn!("토스트 스레드를 시작하지 못했습니다: {e}");
                    None
                }
            }
        }

        fn send(&self, msg: Msg) {
            let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(msg);
            }
        }

        /// 토스트 스레드를 끝낸다(채널을 닫는다).
        pub fn stop(&self) {
            self.tx.lock().unwrap_or_else(|e| e.into_inner()).take();
        }
    }

    fn data_for(p: &ToastProgress, seq: u32) -> windows::core::Result<NotificationData> {
        let data = NotificationData::new()?;
        let values = data.Values()?;
        for (k, v) in progress_values(p) {
            values.Insert(&HSTRING::from(k), &HSTRING::from(v))?;
        }
        data.SetSequenceNumber(seq)?;
        Ok(data)
    }

    fn run(n: &WinNotifier, action: &ToastAction, seq: u32) -> windows::core::Result<()> {
        let group = HSTRING::from(TOAST_GROUP);
        match action {
            ToastAction::Show(c) => {
                let doc = XmlDocument::new()?;
                doc.LoadXml(&HSTRING::from(toast_xml(c)))?;
                let toast = ToastNotification::CreateToastNotification(&doc)?;
                toast.SetTag(&HSTRING::from(c.tag.as_str()))?;
                toast.SetGroup(&group)?;
                if let Some(p) = &c.progress {
                    toast.SetData(&data_for(p, seq)?)?;
                }
                n.Show(&toast)
            }
            ToastAction::Update { tag, progress } => {
                let data = data_for(progress, seq)?;
                n.UpdateWithTagAndGroup(&data, &HSTRING::from(tag.as_str()), &group)?;
                Ok(())
            }
            ToastAction::Remove { tag } => ToastNotificationManager::History()?
                .RemoveGroupedTagWithId(
                    &HSTRING::from(tag.as_str()),
                    &group,
                    &HSTRING::from(AUMID),
                ),
        }
    }

    impl Notifier for ToastNotifier {
        fn event(&self, ev: &JobEvent) {
            dispatch(&LogNotifier, ev);
            self.send(Msg::Event(ev.clone()));
        }
        fn error(&self, error: &ErrorInfo) {
            LogNotifier.error(error);
            self.send(Msg::Error(error.clone()));
        }
        fn shutdown(&self) {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Record(Mutex<Vec<String>>);
    impl Notifier for Record {
        fn job_started(&self, _: BatchId, j: JobId) {
            self.0.lock().unwrap().push(format!("start {j}"));
        }
        fn job_done(&self, j: JobId, _: &Path, o: &Path) {
            self.0
                .lock()
                .unwrap()
                .push(format!("done {j} {}", o.display()));
        }
        fn job_failed(&self, j: JobId, _: &Path, e: &ErrorInfo) {
            self.0
                .lock()
                .unwrap()
                .push(format!("fail {j} {:?}", e.code));
        }
        fn error(&self, e: &ErrorInfo) {
            self.0.lock().unwrap().push(format!("error {:?}", e.code));
        }
    }

    fn err(msg: &str) -> ErrorInfo {
        let mut e = convtool_core::CoreError::InvalidInput(String::new()).info();
        e.message = msg.into();
        e
    }

    #[test]
    fn dispatch_routes_events() {
        let r = Record::default();
        let e = err("x");
        r.event(&JobEvent::Started {
            batch_id: 1,
            job_id: 2,
            engine: convtool_core::EngineId::Vips,
        });
        r.event(&JobEvent::Done {
            batch_id: 1,
            job_id: 2,
            input: "a.png".into(),
            output: "a.webp".into(),
        });
        r.event(&JobEvent::Failed {
            batch_id: 1,
            job_id: 3,
            input: "b.png".into(),
            error: e.clone(),
        });
        r.event(&JobEvent::Rejected {
            batch_id: 1,
            input: "c.xyz".into(),
            error: e,
        });
        LogNotifier.event(&JobEvent::Canceled {
            batch_id: 1,
            job_id: 4,
            input: "d".into(),
        });
        assert_eq!(
            *r.0.lock().unwrap(),
            vec![
                "start 2".to_string(),
                "done 2 a.webp".to_string(),
                "fail 3 InvalidInput".to_string(),
                "error InvalidInput".to_string(),
            ]
        );
    }

    fn queued(b: BatchId, j: JobId, input: &str) -> JobEvent {
        JobEvent::Queued {
            batch_id: b,
            job_id: j,
            input: input.into(),
            to: "webp".into(),
        }
    }
    fn started(b: BatchId, j: JobId) -> JobEvent {
        JobEvent::Started {
            batch_id: b,
            job_id: j,
            engine: convtool_core::EngineId::Vips,
        }
    }
    fn batch_done(b: BatchId, s: u32, f: u32, c: u32, r: u32) -> JobEvent {
        JobEvent::BatchDone {
            batch_id: b,
            succeeded: s,
            failed: f,
            canceled: c,
            rejected: r,
            exit_code: 0,
        }
    }

    #[test]
    fn planner_single_file_success() {
        let mut p = ToastPlanner::new();
        let t0 = Instant::now();
        assert_eq!(p.on_event(&queued(1, 1, r"C:\v\a.png"), t0), None);
        let Some(ToastAction::Show(c)) = p.on_event(&started(1, 1), t0) else {
            panic!("시작 알림");
        };
        assert_eq!(c.tag, "batch-1");
        assert_eq!(c.title, "변환 중");
        assert_eq!(c.body, "a.png");
        assert_eq!(c.progress.as_ref().unwrap().value_text, "0/1");

        // 갱신 간격 안의 진행률은 건너뛴다
        let prog = |v| JobEvent::Progress {
            batch_id: 1,
            job_id: 1,
            progress: Some(v),
        };
        assert_eq!(
            p.on_event(&prog(0.2), t0 + Duration::from_millis(100)),
            None
        );
        let Some(ToastAction::Update { tag, progress }) =
            p.on_event(&prog(0.5), t0 + Duration::from_millis(600))
        else {
            panic!("진행 갱신");
        };
        assert_eq!(tag, "batch-1");
        assert!((progress.value - 0.5).abs() < 1e-9);

        let done = JobEvent::Done {
            batch_id: 1,
            job_id: 1,
            input: r"C:\v\a.png".into(),
            output: r"C:\v\a.webp".into(),
        };
        let Some(ToastAction::Update { progress, .. }) = p.on_event(&done, t0) else {
            panic!("완료 갱신");
        };
        assert_eq!(progress.value_text, "1/1");
        let Some(ToastAction::Show(c)) = p.on_event(&batch_done(1, 1, 0, 0, 0), t0) else {
            panic!("완료 알림");
        };
        assert_eq!(c.tag, "batch-1");
        assert_eq!(c.title, "변환 완료");
        assert_eq!(c.body, "a.webp");
        assert_eq!(c.open_folder, Some(PathBuf::from(r"C:\v")));
        assert!(c.progress.is_none());
    }

    #[test]
    fn planner_partial_failure_and_rejected_only() {
        let mut p = ToastPlanner::new();
        let t0 = Instant::now();
        p.on_event(&queued(2, 1, "a.png"), t0);
        p.on_event(&queued(2, 2, "b.png"), t0);
        p.on_event(&started(2, 1), t0);
        p.on_event(
            &JobEvent::Done {
                batch_id: 2,
                job_id: 1,
                input: "a.png".into(),
                output: r"D:\out\a.webp".into(),
            },
            t0,
        );
        p.on_event(
            &JobEvent::Failed {
                batch_id: 2,
                job_id: 2,
                input: "b.png".into(),
                error: err("손상된 파일"),
            },
            t0,
        );
        let Some(ToastAction::Show(c)) = p.on_event(&batch_done(2, 1, 1, 0, 0), t0) else {
            panic!();
        };
        assert_eq!(c.title, "일부 변환 실패");
        assert_eq!(c.body, "1개 완료, 1개 실패: 손상된 파일");
        assert_eq!(c.open_folder, Some(PathBuf::from(r"D:\out")));

        // 큐에 못 들어간 배치: 진행 알림 없이 실패만
        p.on_event(
            &JobEvent::Rejected {
                batch_id: 3,
                input: "x.xyz".into(),
                error: err("지원하지 않는 형식"),
            },
            t0,
        );
        let Some(ToastAction::Show(c)) = p.on_event(&batch_done(3, 0, 0, 0, 1), t0) else {
            panic!();
        };
        assert_eq!(c.title, "변환 실패");
        assert_eq!(c.body, "지원하지 않는 형식");
        assert_eq!(c.open_folder, None);
    }

    #[test]
    fn planner_all_canceled_removes() {
        let mut p = ToastPlanner::new();
        let t0 = Instant::now();
        p.on_event(&queued(4, 1, "a.mov"), t0);
        p.on_event(&started(4, 1), t0);
        p.on_event(
            &JobEvent::Canceled {
                batch_id: 4,
                job_id: 1,
                input: "a.mov".into(),
            },
            t0,
        );
        assert_eq!(
            p.on_event(&batch_done(4, 0, 0, 1, 0), t0),
            Some(ToastAction::Remove {
                tag: "batch-4".into()
            })
        );
        // 시작 전에 모두 취소: 아무것도 띄우지 않는다
        p.on_event(&queued(5, 2, "b.mov"), t0);
        assert_eq!(p.on_event(&batch_done(5, 0, 0, 1, 0), t0), None);
    }

    #[test]
    fn planner_multi_file_body_and_error_tags() {
        let mut p = ToastPlanner::new();
        let t0 = Instant::now();
        p.on_event(&queued(6, 1, "a.png"), t0);
        p.on_event(&queued(6, 2, "b.png"), t0);
        p.on_event(&queued(6, 3, "c.png"), t0);
        let Some(ToastAction::Show(c)) = p.on_event(&started(6, 1), t0) else {
            panic!();
        };
        assert_eq!(c.body, "파일 3개");
        let ToastAction::Show(a) = p.on_error(&err("엔진을 확인하는 중")) else {
            panic!();
        };
        let ToastAction::Show(b) = p.on_error(&err("x")) else {
            panic!();
        };
        assert_ne!(a.tag, b.tag);
        assert_eq!(a.body, "엔진을 확인하는 중");
    }

    #[test]
    fn xml_and_uri() {
        assert_eq!(
            xml_escape(r#"a<b>&"c"'d"#),
            "a&lt;b&gt;&amp;&quot;c&quot;&apos;d"
        );
        assert_eq!(xml_escape("a\u{1}b\nc"), "ab\nc");
        assert_eq!(
            folder_file_uri(Path::new(r"C:\영상\a b")),
            "file:///C:/%EC%98%81%EC%83%81/a%20b/"
        );
        assert_eq!(
            folder_file_uri(Path::new(r"\\nas\공유")),
            "file://nas/%EA%B3%B5%EC%9C%A0/"
        );
        assert_eq!(folder_file_uri(Path::new(r"C:\")), "file:///C:/");
        assert_eq!(
            folder_file_uri(Path::new(r"C:\a&b#")),
            "file:///C:/a%26b%23/"
        );
    }

    #[test]
    fn toast_xml_shape() {
        let c = ToastContent {
            tag: "batch-1".into(),
            title: "변환 완료".into(),
            body: "a&b.webp".into(),
            progress: None,
            open_folder: Some(PathBuf::from(r"C:\v")),
        };
        let x = toast_xml(&c);
        assert!(x.starts_with(r#"<toast><visual><binding template="ToastGeneric">"#));
        assert!(x.contains("<text>a&amp;b.webp</text>"));
        assert!(x.contains(r#"activationType="protocol" arguments="file:///C:/v/""#));
        assert!(!x.contains("<progress"));
        let mut c2 = c.clone();
        c2.progress = Some(ToastProgress {
            value: 0.25,
            value_text: "1/4".into(),
            status: "변환 중".into(),
        });
        c2.open_folder = None;
        let x2 = toast_xml(&c2);
        assert!(x2.contains(r#"value="{progressValue}""#));
        assert!(!x2.contains("<actions>"));
        let v = progress_values(c2.progress.as_ref().unwrap());
        assert_eq!(v[0], ("progressValue", "0.250".to_string()));
        assert_eq!(v[1].1, "1/4");
    }
}
