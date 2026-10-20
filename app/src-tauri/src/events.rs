//! Tauri 이벤트 이름과 payload 타입 (Rust → UI).
//!
//! serde만 쓰므로 tauri feature 없이도 컴파일된다. `app/ui/src/shared/ipc.ts`와 1:1이다.
//! 규칙: 필드 snake_case, `Option`은 항상 필드가 있고 없으면 `null`, `skip_serializing_if` 금지.

use convtool_core::FormatsReport;
use serde::{Deserialize, Serialize};

pub const WINDOW_MAIN: &str = "main";
pub const WINDOW_RADIAL: &str = "radial";

pub const EV_RADIAL_OPEN: &str = "radial:open";
pub const EV_RADIAL_CLOSE: &str = "radial:close";
pub const EV_RADIAL_MBUTTON_UP: &str = "radial:mbutton_up";
pub const EV_JOB: &str = "job:event";
pub const EV_THEME: &str = "theme:changed";
pub const EV_MAIN_NAVIGATE: &str = "main:navigate";

/// 오버레이 창 기준 CSS px
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidePosition {
    Below,
    Above,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenedBy {
    Wheel,
    ContextMenu,
    Hotkey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeInfo {
    /// "#RRGGBB"
    pub accent: Option<String>,
    pub animations_enabled: bool,
    pub high_contrast: bool,
}

impl Default for ThemeInfo {
    fn default() -> Self {
        Self {
            accent: None,
            animations_enabled: true,
            high_contrast: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadialOpenPayload {
    pub session_id: u64,
    pub report: FormatsReport,
    pub center: Point,
    pub guide_position: GuidePosition,
    pub theme: ThemeInfo,
    pub opened_by: OpenedBy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    FocusLost,
    Superseded,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadialClosePayload {
    pub session_id: u64,
    pub reason: CloseReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MButtonUpPayload {
    pub session_id: u64,
    pub x: f64,
    pub y: f64,
    pub held_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MainView {
    Drop,
    History,
    Settings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MainNavigatePayload {
    pub view: MainView,
}

#[cfg(test)]
mod tests {
    use super::*;
    use convtool_core::{RadialLayout, RadialSlot};
    use serde_json::json;

    fn empty_report() -> FormatsReport {
        FormatsReport {
            files: vec![],
            skipped: vec![],
            options: vec![],
            layout: RadialLayout {
                slot_count: 4,
                slots: vec![
                    RadialSlot::Empty,
                    RadialSlot::Empty,
                    RadialSlot::More,
                    RadialSlot::Format {
                        format_id: "mp4".into(),
                    },
                ],
                more: vec![],
            },
        }
    }

    #[test]
    fn radial_open_payload_json_shape() {
        let p = RadialOpenPayload {
            session_id: 3,
            report: empty_report(),
            center: Point { x: 240.0, y: 240.0 },
            guide_position: GuidePosition::Below,
            theme: ThemeInfo {
                accent: None,
                animations_enabled: false,
                high_contrast: true,
            },
            opened_by: OpenedBy::ContextMenu,
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["session_id"], 3);
        assert_eq!(v["center"], json!({"x": 240.0, "y": 240.0}));
        assert_eq!(v["guide_position"], "below");
        assert_eq!(v["opened_by"], "context_menu");
        // Option은 null로 남는다 (skip 금지)
        assert_eq!(v["theme"]["accent"], serde_json::Value::Null);
        assert_eq!(v["theme"]["animations_enabled"], false);
        assert_eq!(v["theme"]["high_contrast"], true);
        assert_eq!(v["report"]["layout"]["slots"][2], json!({"kind": "more"}));
        assert_eq!(
            v["report"]["layout"]["slots"][3],
            json!({"kind": "format", "format_id": "mp4"})
        );
        let back: RadialOpenPayload = serde_json::from_value(v).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn close_and_mbutton_and_navigate_json() {
        let c = RadialClosePayload {
            session_id: 9,
            reason: CloseReason::FocusLost,
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({"session_id": 9, "reason": "focus_lost"})
        );
        let s = serde_json::to_value(CloseReason::Superseded).unwrap();
        assert_eq!(s, "superseded");
        assert_eq!(
            serde_json::to_value(CloseReason::Shutdown).unwrap(),
            "shutdown"
        );

        let m = MButtonUpPayload {
            session_id: 1,
            x: 12.5,
            y: 300.0,
            held_ms: 180,
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            json!({"session_id": 1, "x": 12.5, "y": 300.0, "held_ms": 180})
        );

        let n = MainNavigatePayload {
            view: MainView::Settings,
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({"view": "settings"})
        );
        assert_eq!(serde_json::to_value(MainView::Drop).unwrap(), "drop");
        assert_eq!(serde_json::to_value(MainView::History).unwrap(), "history");
        assert_eq!(serde_json::to_value(OpenedBy::Wheel).unwrap(), "wheel");
        assert_eq!(serde_json::to_value(OpenedBy::Hotkey).unwrap(), "hotkey");
        assert_eq!(serde_json::to_value(GuidePosition::Above).unwrap(), "above");
    }

    #[test]
    fn event_names_match_ipc_ts() {
        assert_eq!(EV_RADIAL_OPEN, "radial:open");
        assert_eq!(EV_RADIAL_CLOSE, "radial:close");
        assert_eq!(EV_RADIAL_MBUTTON_UP, "radial:mbutton_up");
        assert_eq!(EV_JOB, "job:event");
        assert_eq!(EV_THEME, "theme:changed");
        assert_eq!(EV_MAIN_NAVIGATE, "main:navigate");
        assert_eq!(WINDOW_MAIN, "main");
        assert_eq!(WINDOW_RADIAL, "radial");
    }
}
