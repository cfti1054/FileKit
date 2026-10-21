//! 원형 메뉴 세션 관리 (순수 로직). `overlay.rs`가 창 조작과 함께 쓴다.
//!
//! 세션 계약 (설계 계획 (3)):
//! - 새 메뉴를 열면 새 세션 ID. 열린 세션이 있으면 그 세션은 `superseded`로 닫힌다.
//! - `radial_select`/`radial_cancel`은 현재 세션 ID일 때만 효과가 있다. 오래된 ID는 무시한다.
//! - `radial_ready`는 열린 세션이 있으면 그 payload를 돌려준다 (UI가 이벤트를 놓쳤을 때).

use std::path::PathBuf;
use std::time::Instant;

use crate::events::{MButtonUpPayload, OpenedBy, RadialOpenPayload};
use crate::placement::to_overlay_css;

#[derive(Debug, Clone, PartialEq)]
pub struct OpenSession {
    pub payload: RadialOpenPayload,
    /// 변환 대상 (report.files의 경로. 건너뛴 파일은 뺀다)
    pub files: Vec<PathBuf>,
    /// 오버레이 창 왼쪽 위 (물리 px)
    pub window_origin: (i32, i32),
    pub scale: f64,
    /// 휠 버튼을 누른 시각 (제스처 held_ms 계산). 휠로 연 경우만.
    pub down_at: Option<Instant>,
    /// 이 세션을 연 뒤 창이 포커스를 받았는지. 받기 전의 "포커스 잃음"은 앞 세션의 숨기기에서 온
    /// 늦은 이벤트일 수 있으므로 무시한다.
    pub focused: bool,
}

impl OpenSession {
    pub fn id(&self) -> u64 {
        self.payload.session_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectOutcome {
    /// 현재 세션이 아님 → 무시
    Stale,
    /// 메뉴에 없는 형식
    UnknownFormat,
    /// 사용 불가 형식 (UI가 막아야 하지만 방어)
    Unavailable(String),
    /// 이 파일들을 `to`로 변환한다
    Submit { files: Vec<PathBuf>, to: String },
}

#[derive(Debug, Default)]
pub struct Sessions {
    last_id: u64,
    current: Option<OpenSession>,
}

impl Sessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// 새 세션 ID를 받는다 (1부터)
    pub fn allocate_id(&mut self) -> u64 {
        self.last_id += 1;
        self.last_id
    }

    /// 세션을 연다. 이미 열린 세션이 있으면 그 ID를 돌려준다 (`radial:close{superseded}` 대상).
    pub fn open(&mut self, session: OpenSession) -> Option<u64> {
        self.current.replace(session).map(|s| s.id())
    }

    pub fn current(&self) -> Option<&OpenSession> {
        self.current.as_ref()
    }

    pub fn current_id(&self) -> Option<u64> {
        self.current.as_ref().map(OpenSession::id)
    }

    /// `radial_ready`: 열린 세션의 payload
    pub fn ready_payload(&self) -> Option<RadialOpenPayload> {
        self.current.as_ref().map(|s| s.payload.clone())
    }

    /// `radial_select`. 현재 세션이면 닫고 결과를 돌려준다.
    pub fn select(&mut self, session_id: u64, format_id: &str) -> SelectOutcome {
        if self.current_id() != Some(session_id) {
            return SelectOutcome::Stale;
        }
        let Some(s) = self.current.take() else {
            return SelectOutcome::Stale;
        };
        match s.payload.report.options.iter().find(|o| o.id == format_id) {
            None => SelectOutcome::UnknownFormat,
            Some(o) if !o.available => SelectOutcome::Unavailable(
                o.unavailable
                    .as_ref()
                    .map(|u| u.message.clone())
                    .unwrap_or_else(|| format!("{} 형식을 지금 쓸 수 없습니다", o.label)),
            ),
            Some(o) => SelectOutcome::Submit {
                files: s.files,
                to: o.id.clone(),
            },
        }
    }

    /// `radial_cancel`. 현재 세션이었으면 true.
    pub fn cancel(&mut self, session_id: u64) -> bool {
        if self.current_id() == Some(session_id) {
            self.current = None;
            true
        } else {
            false
        }
    }

    /// 오버레이 창이 포커스를 받았다
    pub fn mark_focused(&mut self) {
        if let Some(s) = self.current.as_mut() {
            s.focused = true;
        }
    }

    /// 오버레이 창이 포커스를 잃었다. 포커스를 받은 적 있는 세션만 닫는다. 닫은 세션 ID.
    pub fn close_on_focus_lost(&mut self) -> Option<u64> {
        if self.current.as_ref().is_some_and(|s| s.focused) {
            self.close_current()
        } else {
            None
        }
    }

    /// Rust가 먼저 닫을 때 (종료 등). 닫은 세션 ID.
    pub fn close_current(&mut self) -> Option<u64> {
        self.current.take().map(|s| s.id())
    }

    /// 훅이 받은 휠 버튼 떼기 → `radial:mbutton_up` payload. 휠로 연 세션이고, 그 세션을 연 누름
    /// (`press_down` = 누른 시각)의 짝일 때만. 다른 파일을 휠 클릭하는 동안 열려 있던 이전 메뉴가
    /// 새 누름의 떼기를 제스처 놓기로 받지 않게 한다.
    pub fn mbutton_up(
        &self,
        screen: (i32, i32),
        press_down: Instant,
        at: Instant,
    ) -> Option<MButtonUpPayload> {
        let s = self.current.as_ref()?;
        if s.payload.opened_by != OpenedBy::Wheel {
            return None;
        }
        let down = s.down_at.filter(|d| *d == press_down)?;
        let p = to_overlay_css(screen, s.window_origin, s.scale);
        let held = at.saturating_duration_since(down).as_millis();
        Some(MButtonUpPayload {
            session_id: s.id(),
            x: p.x,
            y: p.y,
            held_ms: u32::try_from(held).unwrap_or(u32::MAX),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{GuidePosition, Point, ThemeInfo};
    use convtool_core::{
        Category, EngineId, FileFormat, FormatOption, FormatsReport, RadialLayout, RadialSlot,
        Unavailable, UnavailableCode,
    };
    use std::time::Duration;

    fn opt(id: &str, available: bool) -> FormatOption {
        FormatOption {
            id: id.into(),
            label: id.to_uppercase(),
            description: String::new(),
            engines: vec![EngineId::Office],
            available,
            unavailable: (!available).then(|| Unavailable {
                code: UnavailableCode::EngineMissing,
                engine: Some(EngineId::Office),
                message: "LibreOffice가 설치되지 않았습니다".into(),
            }),
        }
    }

    fn session(id: u64, opened_by: OpenedBy) -> OpenSession {
        let file = PathBuf::from(r"C:\문서\보고서.docx");
        OpenSession {
            payload: RadialOpenPayload {
                session_id: id,
                report: FormatsReport {
                    files: vec![FileFormat {
                        path: file.clone(),
                        format: "docx".into(),
                        category: Category::Document,
                    }],
                    skipped: vec![],
                    options: vec![opt("pdf", false), opt("txt", true)],
                    layout: RadialLayout {
                        slot_count: 4,
                        slots: vec![RadialSlot::Empty; 4],
                        more: vec![],
                    },
                },
                center: Point { x: 240.0, y: 240.0 },
                guide_position: GuidePosition::Below,
                theme: ThemeInfo::default(),
                opened_by,
            },
            files: vec![file],
            window_origin: (100, 200),
            scale: 1.5,
            down_at: Some(Instant::now()),
            focused: false,
        }
    }

    #[test]
    fn ids_increase_from_one() {
        let mut s = Sessions::new();
        assert_eq!(s.allocate_id(), 1);
        assert_eq!(s.allocate_id(), 2);
    }

    #[test]
    fn open_supersedes_previous() {
        let mut s = Sessions::new();
        assert_eq!(s.open(session(1, OpenedBy::Wheel)), None);
        assert_eq!(s.open(session(2, OpenedBy::Wheel)), Some(1));
        assert_eq!(s.current_id(), Some(2));
    }

    #[test]
    fn stale_select_and_cancel_are_ignored() {
        let mut s = Sessions::new();
        s.open(session(1, OpenedBy::Wheel));
        s.open(session(2, OpenedBy::Wheel));
        assert_eq!(s.select(1, "txt"), SelectOutcome::Stale);
        assert!(!s.cancel(1));
        assert_eq!(s.current_id(), Some(2));
    }

    #[test]
    fn select_available_submits_and_closes() {
        let mut s = Sessions::new();
        s.open(session(5, OpenedBy::ContextMenu));
        assert_eq!(
            s.select(5, "txt"),
            SelectOutcome::Submit {
                files: vec![PathBuf::from(r"C:\문서\보고서.docx")],
                to: "txt".into()
            }
        );
        assert_eq!(s.current_id(), None);
        // 두 번째 고르기는 무시
        assert_eq!(s.select(5, "txt"), SelectOutcome::Stale);
    }

    #[test]
    fn select_unavailable_or_unknown_closes_with_reason() {
        let mut s = Sessions::new();
        s.open(session(1, OpenedBy::Wheel));
        assert_eq!(
            s.select(1, "pdf"),
            SelectOutcome::Unavailable("LibreOffice가 설치되지 않았습니다".into())
        );
        assert_eq!(s.current_id(), None);
        s.open(session(2, OpenedBy::Wheel));
        assert_eq!(s.select(2, "mp4"), SelectOutcome::UnknownFormat);
        assert_eq!(s.current_id(), None);
    }

    #[test]
    fn cancel_and_close() {
        let mut s = Sessions::new();
        s.open(session(1, OpenedBy::Wheel));
        assert!(s.cancel(1));
        assert_eq!(s.close_current(), None);
        s.open(session(2, OpenedBy::Wheel));
        assert_eq!(s.close_current(), Some(2));
        assert_eq!(s.ready_payload(), None);
    }

    #[test]
    fn focus_lost_before_focus_is_ignored() {
        let mut s = Sessions::new();
        s.open(session(1, OpenedBy::Wheel));
        // 앞 세션을 숨길 때 생긴 늦은 포커스 잃음
        assert_eq!(s.close_on_focus_lost(), None);
        assert_eq!(s.current_id(), Some(1));
        s.mark_focused();
        assert_eq!(s.close_on_focus_lost(), Some(1));
        assert_eq!(s.current_id(), None);
        s.mark_focused();
        assert_eq!(s.close_on_focus_lost(), None);
    }

    #[test]
    fn ready_returns_open_payload() {
        let mut s = Sessions::new();
        assert_eq!(s.ready_payload(), None);
        s.open(session(3, OpenedBy::Wheel));
        assert_eq!(s.ready_payload().map(|p| p.session_id), Some(3));
    }

    #[test]
    fn mbutton_up_converts_to_css_and_measures_hold() {
        let mut s = Sessions::new();
        let mut sess = session(7, OpenedBy::Wheel);
        let down = Instant::now();
        sess.down_at = Some(down);
        s.open(sess);
        let p = s
            .mbutton_up(
                (100 + 360, 200 + 360),
                down,
                down + Duration::from_millis(180),
            )
            .unwrap();
        assert_eq!(p.session_id, 7);
        assert_eq!((p.x, p.y), (240.0, 240.0));
        assert_eq!(p.held_ms, 180);
        // 다른 누름(이전 메뉴가 열린 채 새 휠 클릭)의 떼기는 넘기지 않는다
        assert!(s
            .mbutton_up(
                (0, 0),
                down + Duration::from_millis(5),
                down + Duration::from_millis(9)
            )
            .is_none());
    }

    #[test]
    fn mbutton_up_only_for_wheel_sessions() {
        let mut s = Sessions::new();
        let now = Instant::now();
        assert!(s.mbutton_up((0, 0), now, now).is_none());
        s.open(session(1, OpenedBy::ContextMenu));
        let now = Instant::now();
        assert!(s.mbutton_up((0, 0), now, now).is_none());
    }
}
