//! 시스템 테마 읽기: 강조색, 애니메이션 효과, 고대비.
//!
//! 변경 감시: `watch`(숨은 창이 WM_SETTINGCHANGE·WM_DWMCOLORIZATIONCOLORCHANGED를 받는다).
//! 레지스트리는 **읽기만** 한다 (`HKCU\Software\Microsoft\Windows\DWM\AccentColor`).
//! 밝게/어둡게는 webview의 `prefers-color-scheme`이 따르므로 여기서 읽지 않는다.

use crate::events::ThemeInfo;

/// DWM `AccentColor` DWORD(ABGR, 0xAABBGGRR) → `"#RRGGBB"`
pub fn accent_from_abgr(abgr: u32) -> String {
    let r = abgr & 0xFF;
    let g = (abgr >> 8) & 0xFF;
    let b = (abgr >> 16) & 0xFF;
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// 지금 시스템 테마. Windows가 아니면 기본값.
pub fn current() -> ThemeInfo {
    #[cfg(windows)]
    {
        ThemeInfo {
            accent: win::accent_color().map(accent_from_abgr),
            animations_enabled: win::client_area_animation().unwrap_or(true),
            high_contrast: win::high_contrast().unwrap_or(false),
        }
    }
    #[cfg(not(windows))]
    {
        ThemeInfo::default()
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    use windows::core::w;
    use windows::Win32::Foundation::{BOOL, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };

    /// HKCU DWM AccentColor (읽기 전용)
    pub fn accent_color() -> Option<u32> {
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: value/size는 유효한 출력 버퍼다. 읽기만 한다.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Windows\DWM"),
                w!("AccentColor"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut c_void),
                Some(&mut size),
            )
        };
        (r == ERROR_SUCCESS).then_some(value)
    }

    /// 제어판 "애니메이션 효과"
    pub fn client_area_animation() -> Option<bool> {
        let mut on = BOOL(1);
        // SAFETY: SPI_GETCLIENTAREAANIMATION은 BOOL 하나를 쓴다.
        let r = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some(&mut on as *mut BOOL as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        r.ok().map(|_| on.as_bool())
    }

    pub fn high_contrast() -> Option<bool> {
        let mut hc = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        // SAFETY: SPI_GETHIGHCONTRAST는 cbSize를 채운 HIGHCONTRASTW를 받는다.
        let r = unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                hc.cbSize,
                Some(&mut hc as *mut HIGHCONTRASTW as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        r.ok().map(|_| hc.dwFlags.contains(HCF_HIGHCONTRASTON))
    }
}

/// 시스템 테마 변경 알림을 받는 숨은 창이 테마 관련 메시지로 볼 것인지 (순수)
pub fn is_theme_message(msg: u32) -> bool {
    // WM_SETTINGCHANGE(0x1A), WM_SYSCOLORCHANGE(0x15), WM_THEMECHANGED(0x31A),
    // WM_DWMCOLORIZATIONCOLORCHANGED(0x320)
    matches!(msg, 0x001A | 0x0015 | 0x031A | 0x0320)
}

#[cfg(windows)]
pub use watch::{watch, ThemeWatcher};

/// `WM_SETTINGCHANGE`·강조색 변경을 받아 바로 알린다. broadcast 메시지는 message-only 창에 오지 않으므로
/// 보이지 않는 최상위 창(WS_EX_TOOLWINDOW, 표시 안 함)을 전용 스레드에 만든다.
#[cfg(windows)]
mod watch {
    use std::cell::RefCell;
    use std::io;
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PostThreadMessageW, RegisterClassExW, TranslateMessage, MSG, WINDOW_STYLE, WM_QUIT,
        WNDCLASSEXW, WS_EX_TOOLWINDOW,
    };

    type Callback = Box<dyn Fn()>;

    thread_local! {
        static ON_CHANGE: RefCell<Option<Callback>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if super::is_theme_message(msg) {
            ON_CHANGE.with(|c| {
                if let Ok(c) = c.try_borrow() {
                    if let Some(f) = c.as_ref() {
                        f();
                    }
                }
            });
        }
        // SAFETY: 받은 인자를 그대로 기본 처리에 넘긴다.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    pub struct ThemeWatcher {
        thread_id: u32,
        join: Option<JoinHandle<()>>,
    }

    /// 감시 스레드를 시작한다. `on_change`는 감시 스레드에서 불린다(같은 변경에 여러 번 올 수 있다).
    pub fn watch<F: Fn() + Send + 'static>(on_change: F) -> io::Result<ThemeWatcher> {
        let (id_tx, id_rx) = mpsc::channel::<io::Result<u32>>();
        let join = std::thread::Builder::new()
            .name("convtool-theme".into())
            .spawn(move || {
                ON_CHANGE.with(|c| *c.borrow_mut() = Some(Box::new(on_change)));
                let class = w!("FileKitThemeWatcher");
                // SAFETY: 이 스레드에서 창 클래스를 등록하고 숨은 창을 만든다. 이미 등록돼 있으면 0(무시).
                let hwnd = unsafe {
                    let hinst = GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default();
                    let wc = WNDCLASSEXW {
                        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                        lpfnWndProc: Some(wnd_proc),
                        hInstance: hinst,
                        lpszClassName: class,
                        ..Default::default()
                    };
                    let _ = RegisterClassExW(&wc);
                    CreateWindowExW(
                        WS_EX_TOOLWINDOW,
                        class,
                        w!("FileKit theme"),
                        WINDOW_STYLE(0),
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        hinst,
                        None,
                    )
                };
                let hwnd = match hwnd {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = id_tx.send(Err(io::Error::other(format!(
                            "테마 감시 창 만들기 실패: {e}"
                        ))));
                        return;
                    }
                };
                // SAFETY: 인자 없는 조회
                let _ = id_tx.send(Ok(unsafe { GetCurrentThreadId() }));
                let mut msg = MSG::default();
                loop {
                    // SAFETY: msg는 유효한 출력 버퍼. WM_QUIT이면 0, 오류면 -1.
                    let r = unsafe { GetMessageW(&mut msg, HWND::default(), 0, 0) };
                    if r.0 == 0 || r.0 == -1 {
                        break;
                    }
                    // SAFETY: 받은 메시지를 그대로 넘긴다.
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                // SAFETY: 이 스레드가 만든 창
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                ON_CHANGE.with(|c| c.borrow_mut().take());
            })?;
        let thread_id = id_rx
            .recv()
            .map_err(|_| io::Error::other("테마 감시 스레드가 끝났습니다"))??;
        Ok(ThemeWatcher {
            thread_id,
            join: Some(join),
        })
    }

    impl ThemeWatcher {
        pub fn stop(mut self) {
            self.stop_inner();
        }

        fn stop_inner(&mut self) {
            if let Some(j) = self.join.take() {
                // SAFETY: 메시지 큐가 있는 스레드에 WM_QUIT을 보낸다.
                if unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
                    .is_ok()
                {
                    let _ = j.join();
                }
            }
        }
    }

    impl Drop for ThemeWatcher {
        fn drop(&mut self) {
            self.stop_inner();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abgr_to_rgb_hex() {
        // Windows 기본 파랑 #0078D4 → ABGR 0xFFD47800
        assert_eq!(accent_from_abgr(0xFFD4_7800), "#0078D4");
        assert_eq!(accent_from_abgr(0x0000_00FF), "#FF0000");
        assert_eq!(accent_from_abgr(0x0000_FF00), "#00FF00");
        assert_eq!(accent_from_abgr(0x00FF_0000), "#0000FF");
        // 알파는 버린다
        assert_eq!(accent_from_abgr(0x8012_3456), "#563412");
        assert_eq!(accent_from_abgr(0), "#000000");
    }

    #[test]
    fn current_does_not_panic() {
        let t = current();
        if let Some(a) = t.accent {
            assert_eq!(a.len(), 7);
            assert!(a.starts_with('#'));
        }
    }

    #[test]
    fn theme_messages() {
        assert!(is_theme_message(0x001A));
        assert!(is_theme_message(0x0320));
        assert!(is_theme_message(0x031A));
        assert!(is_theme_message(0x0015));
        assert!(!is_theme_message(0x0010));
        assert!(!is_theme_message(0x0200));
    }

    #[cfg(windows)]
    #[test]
    fn watcher_starts_and_stops() {
        let w = watch(|| {}).unwrap();
        w.stop();
    }
}
