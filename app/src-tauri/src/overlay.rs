//! 원형 메뉴 오버레이 창 (`radial`).
//!
//! - 트레이 모드 시작 시 `tauri.conf.json`의 radial 설정(`create:false`)으로 미리 만들어 숨겨 둔다.
//! - 열기 순서(세션 계약 1): query_formats → 위치 계산 → set_position(물리) → set_size(논리 480) →
//!   `radial:open` → show → set_focus. 소요 시간은 `tracing`으로 남긴다(100ms 목표 측정용).
//! - 포커스를 잃으면 `radial:close{focus_lost}`를 보내고 숨긴다.

use std::path::PathBuf;
use std::time::Instant;

use convtool_core::{query_formats, CoreError, ErrorInfo};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow, WindowEvent,
};

use crate::events::{
    CloseReason, OpenedBy, Point, RadialClosePayload, RadialOpenPayload, ThemeInfo,
    EV_RADIAL_CLOSE, EV_RADIAL_MBUTTON_UP, EV_RADIAL_OPEN, EV_THEME, WINDOW_RADIAL,
};
use crate::placement::{place_overlay, Rect, CENTER_CSS, OVERLAY_CSS_SIZE};
use crate::session::OpenSession;
use crate::state::{lock, AppState};

pub(crate) fn internal(e: impl std::fmt::Display) -> ErrorInfo {
    CoreError::Internal(e.to_string()).info()
}

/// 미리 만들어 숨겨 둔다 (트레이 모드). 이미 있으면 그대로 쓴다.
pub fn create_radial(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window(WINDOW_RADIAL) {
        return Ok(w);
    }
    let cfg = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == WINDOW_RADIAL)
        .cloned()
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let win = tauri::WebviewWindowBuilder::from_config(app, &cfg)?.build()?;

    let handle = app.clone();
    win.on_window_event(move |e| match e {
        WindowEvent::Focused(true) => {
            lock(&handle.state::<AppState>().sessions).mark_focused();
        }
        WindowEvent::Focused(false) => {
            let closed = lock(&handle.state::<AppState>().sessions).close_on_focus_lost();
            if let Some(id) = closed {
                emit_close(&handle, id, CloseReason::FocusLost);
                hide(&handle);
            }
        }
        _ => {}
    });

    #[cfg(windows)]
    if let Ok(hwnd) = win.hwnd() {
        ex_style::check_and_fix(hwnd.0 as isize);
    }
    Ok(win)
}

fn emit_close(app: &AppHandle, session_id: u64, reason: CloseReason) {
    let payload = RadialClosePayload { session_id, reason };
    if let Err(e) = app.emit_to(WINDOW_RADIAL, EV_RADIAL_CLOSE, payload) {
        tracing::warn!("radial:close 보내기 실패: {e}");
    }
}

/// 오버레이를 숨긴다 (세션 상태는 호출하는 쪽이 정리한다)
pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW_RADIAL) {
        if let Err(e) = w.hide() {
            tracing::warn!("오버레이 숨기기 실패: {e}");
        }
    }
}

/// Rust가 먼저 닫는다 (종료 등)
pub fn close(app: &AppHandle, reason: CloseReason) {
    let closed = lock(&app.state::<AppState>().sessions).close_current();
    if let Some(id) = closed {
        emit_close(app, id, reason);
    }
    hide(app);
}

fn cursor_physical(app: &AppHandle) -> Result<(i32, i32), ErrorInfo> {
    let p = app.cursor_position().map_err(internal)?;
    Ok((p.x.round() as i32, p.y.round() as i32))
}

fn monitor_at(app: &AppHandle, cursor: (i32, i32)) -> (Rect, f64) {
    let m = app
        .monitor_from_point(f64::from(cursor.0), f64::from(cursor.1))
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    match m {
        Some(m) => {
            let wa = m.work_area();
            (
                Rect::from_origin_size(wa.position.x, wa.position.y, wa.size.width, wa.size.height),
                m.scale_factor(),
            )
        }
        None => {
            tracing::warn!("모니터 정보를 얻지 못해 1920×1080, 100%로 가정합니다");
            (
                Rect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                1.0,
            )
        }
    }
}

fn theme_for_open(app: &AppHandle) -> ThemeInfo {
    use std::sync::OnceLock;
    static LAST: OnceLock<std::sync::Mutex<Option<ThemeInfo>>> = OnceLock::new();
    let theme = crate::theme::current();
    let mut last = lock(LAST.get_or_init(Default::default));
    if last.as_ref().is_some_and(|t| *t != theme) {
        // TODO: WM_SETTINGCHANGE를 받아 바로 보내기. 지금은 메뉴를 열 때 바뀐 것을 알린다.
        let _ = app.emit(EV_THEME, theme.clone());
    }
    *last = Some(theme.clone());
    theme
}

/// 원형 메뉴를 연다.
///
/// - `cursor`: 화면 물리 px. None이면 지금 커서 위치(우클릭 `show_menu` 경로).
/// - `down_at`: 휠 버튼을 누른 시각(휠 경로). 클릭→표시 시간을 로그에 남긴다.
pub fn open_menu(
    app: &AppHandle,
    files: Vec<PathBuf>,
    primary: Option<PathBuf>,
    cursor: Option<(i32, i32)>,
    opened_by: OpenedBy,
    down_at: Option<Instant>,
) -> Result<u64, ErrorInfo> {
    let started = Instant::now();
    let _span = tracing::info_span!("open_menu", ?opened_by, files = files.len()).entered();
    let state = app.state::<AppState>();

    let core = state
        .core_now()
        .ok_or_else(|| internal("엔진을 확인하는 중입니다. 잠시 뒤 다시 시도하세요"))?;
    let report = query_formats(&files, primary.as_deref(), &core.engines).map_err(|e| e.info())?;
    let t_query = started.elapsed();

    let cursor = match cursor {
        Some(c) => c,
        None => cursor_physical(app)?,
    };
    let (work_area, scale) = monitor_at(app, cursor);
    let placement = place_overlay(cursor, work_area, scale);

    let win = app
        .get_webview_window(WINDOW_RADIAL)
        .map(Ok)
        .unwrap_or_else(|| create_radial(app))
        .map_err(internal)?;
    win.set_position(PhysicalPosition::new(
        placement.window_x,
        placement.window_y,
    ))
    .map_err(internal)?;
    win.set_size(LogicalSize::new(OVERLAY_CSS_SIZE, OVERLAY_CSS_SIZE))
        .map_err(internal)?;

    let theme = theme_for_open(app);
    let session_files: Vec<PathBuf> = report.files.iter().map(|f| f.path.clone()).collect();

    let (payload, superseded) = {
        let mut sessions = lock(&state.sessions);
        let session_id = sessions.allocate_id();
        let payload = RadialOpenPayload {
            session_id,
            report,
            center: Point {
                x: CENTER_CSS,
                y: CENTER_CSS,
            },
            guide_position: placement.guide,
            theme,
            opened_by,
        };
        let superseded = sessions.open(OpenSession {
            payload: payload.clone(),
            files: session_files,
            window_origin: (placement.window_x, placement.window_y),
            scale: placement.scale,
            down_at,
            focused: false,
        });
        (payload, superseded)
    };
    if let Some(old) = superseded {
        emit_close(app, old, CloseReason::Superseded);
    }

    app.emit_to(WINDOW_RADIAL, EV_RADIAL_OPEN, &payload)
        .map_err(internal)?;
    win.show().map_err(internal)?;
    if let Err(e) = win.set_focus() {
        // 전경 잠금(foreground lock) 때문에 실패할 수 있다. 메뉴는 보이지만 키 입력을 못 받는다.
        tracing::warn!("오버레이 포커스 실패: {e}");
    }

    tracing::info!(
        session_id = payload.session_id,
        query_ms = t_query.as_secs_f64() * 1000.0,
        open_ms = started.elapsed().as_secs_f64() * 1000.0,
        since_click_ms = down_at.map(|t| t.elapsed().as_secs_f64() * 1000.0),
        guide = ?placement.guide,
        scale = placement.scale,
        "원형 메뉴 표시"
    );
    Ok(payload.session_id)
}

/// 훅이 받은 휠 버튼 떼기 → `radial:mbutton_up` (제스처 방식)
pub fn on_mbutton_up(app: &AppHandle, screen: (i32, i32), at: Instant) {
    let payload = lock(&app.state::<AppState>().sessions).mbutton_up(screen, at);
    if let Some(p) = payload {
        if let Err(e) = app.emit_to(WINDOW_RADIAL, EV_RADIAL_MBUTTON_UP, p) {
            tracing::warn!("radial:mbutton_up 보내기 실패: {e}");
        }
    }
}

/// 오버레이 HWND의 확장 스타일 확인 (spec 4장: `WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED`).
#[cfg(windows)]
mod ex_style {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_APPWINDOW, WS_EX_LAYERED,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    };

    /// 필요한 비트 중 빠진 것 (순수)
    pub fn missing(ex: u32) -> u32 {
        let want = WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_LAYERED.0;
        want & !ex
    }

    /// 보정한 스타일 (순수): TOOLWINDOW를 켜고, TOOLWINDOW를 무력화하는 APPWINDOW를 끈다.
    pub fn fixed(ex: u32) -> u32 {
        (ex | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0
    }

    /// 확인하고, 안전하게 더할 수 있는 `WS_EX_TOOLWINDOW`만 더한다(Alt+Tab에서 숨김).
    /// tao가 켜 두는 `WS_EX_APPWINDOW`는 TOOLWINDOW보다 우선하므로 함께 끈다.
    /// `WS_EX_TOPMOST`는 alwaysOnTop이 SetWindowPos(HWND_TOPMOST)로 켠다(보이기 전에는 꺼져 있을 수 있다).
    /// `WS_EX_LAYERED`는 직접 켜지 않는다: SetLayeredWindowAttributes 없이 켜면 창이 보이지 않고,
    /// WebView2 투명 창은 DirectComposition으로 투명도를 처리한다. 빠졌으면 로그로만 남긴다.
    pub fn check_and_fix(raw: isize) {
        let hwnd = HWND(raw as *mut core::ffi::c_void);
        // SAFETY: tauri가 준 유효한 최상위 창 HWND. 스타일 읽기·쓰기만 한다.
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let miss = missing(ex);
            let new = fixed(ex);
            if new != ex {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new as isize);
                let _ = SetWindowPos(
                    hwnd,
                    HWND::default(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
            tracing::info!(
                ex_style = format_args!("{ex:#010x}"),
                missing = format_args!("{miss:#010x}"),
                fixed = format_args!("{new:#010x}"),
                "오버레이 확장 스타일 확인 (TOOLWINDOW만 보정, TOPMOST·LAYERED는 로그)"
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn missing_bits() {
            assert_eq!(
                missing(0),
                WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_LAYERED.0
            );
            assert_eq!(
                missing(WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_LAYERED.0 | 0x10),
                0
            );
            assert_eq!(
                missing(WS_EX_TOPMOST.0),
                WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0
            );
        }

        #[test]
        fn fixed_style() {
            // 실제 관찰값: APPWINDOW | WINDOWEDGE | TOPMOST | ACCEPTFILES
            let seen = 0x0004_0118;
            assert_eq!(fixed(seen), 0x0000_0198);
            assert_eq!(fixed(fixed(seen)), fixed(seen));
        }
    }
}
