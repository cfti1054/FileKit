//! 시스템 테마 읽기: 강조색, 애니메이션 효과, 고대비.
//!
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
}
