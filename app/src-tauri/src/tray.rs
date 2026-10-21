//! 트레이 아이콘과 메뉴: 열기, 원형 메뉴 일시 중지(체크), 설정, 종료. 변환 중에는 바쁨 아이콘.

use std::sync::atomic::Ordering;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::busy::badge_rgba;
use crate::events::{MainNavigatePayload, MainView, EV_MAIN_NAVIGATE, WINDOW_MAIN};
use crate::state::AppState;

pub const TRAY_ID: &str = "convtool";
pub const MENU_OPEN: &str = "open";
pub const MENU_PAUSE_RADIAL: &str = "pause_radial";
pub const MENU_SETTINGS: &str = "settings";
pub const MENU_QUIT: &str = "quit";

const TOOLTIP_IDLE: &str = "FileKit";
const TOOLTIP_BUSY: &str = "FileKit - 변환 중";

fn idle_icon(app: &AppHandle) -> Option<Image<'static>> {
    app.default_window_icon().cloned().map(Image::to_owned)
}

fn busy_icon(app: &AppHandle) -> Option<Image<'static>> {
    let base = app.default_window_icon()?;
    let rgba = badge_rgba(base.rgba(), base.width(), base.height());
    Some(Image::new_owned(rgba, base.width(), base.height()))
}

/// 메인 창을 보이고 앞으로 가져온다
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(WINDOW_MAIN) {
        let _ = w.unminimize();
        if let Err(e) = w.show() {
            tracing::warn!("메인 창 보이기 실패: {e}");
        }
        let _ = w.set_focus();
    }
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, MENU_OPEN, "열기", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(
        app,
        MENU_PAUSE_RADIAL,
        "원형 메뉴 일시 중지",
        true,
        false,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, MENU_SETTINGS, "설정", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "종료", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &pause, &settings, &sep, &quit])?;

    let pause_item = pause.clone();
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(TOOLTIP_IDLE)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            MENU_OPEN => show_main(app),
            MENU_PAUSE_RADIAL => {
                // CheckMenuItem은 누를 때 스스로 체크를 바꾼다. 바뀐 값을 읽어 훅 판정에 넘긴다.
                let paused = pause_item.is_checked().unwrap_or(false);
                app.state::<AppState>()
                    .paused
                    .store(paused, Ordering::Relaxed);
                tracing::info!(paused, "원형 메뉴 일시 중지 변경");
            }
            MENU_SETTINGS => {
                show_main(app);
                let payload = MainNavigatePayload {
                    view: MainView::Settings,
                };
                if let Err(e) = app.emit_to(WINDOW_MAIN, EV_MAIN_NAVIGATE, payload) {
                    tracing::warn!("main:navigate 보내기 실패: {e}");
                }
            }
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = idle_icon(app) {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

/// 바쁨 상태에 맞춰 아이콘과 툴팁을 바꾼다
pub fn set_busy(app: &AppHandle, busy: bool) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return; // 창 모드에는 트레이가 없다
    };
    let icon = if busy { busy_icon(app) } else { idle_icon(app) };
    if let Err(e) = tray.set_icon(icon) {
        tracing::warn!("트레이 아이콘 바꾸기 실패: {e}");
    }
    let _ = tray.set_tooltip(Some(if busy { TOOLTIP_BUSY } else { TOOLTIP_IDLE }));
}
