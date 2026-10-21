//! 파이프 보안 설명자: 현재 사용자만 접근 (spec 4장 "현재 사용자만").
//!
//! SDDL `D:P(A;;GA;;;<사용자 SID>)` — 상속 없는(P) DACL에 현재 사용자 모든 권한 하나만 둔다.
//! 기본 DACL(Everyone 읽기)을 쓰지 않으므로 같은 PC의 다른 사용자는 연결하지 못한다.

/// 현재 사용자만 허용하는 SDDL (순수)
pub fn user_only_sddl(user_sid: &str) -> String {
    format!("D:P(A;;GA;;;{user_sid})")
}

#[cfg(windows)]
pub use win::PipeSecurity;

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::io;

    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn os(e: windows::core::Error) -> io::Error {
        io::Error::other(e.to_string())
    }

    /// 현재 프로세스 사용자 SID 문자열 (`S-1-5-21-…`)
    pub fn current_user_sid() -> io::Result<String> {
        let mut token = HANDLE::default();
        // SAFETY: 현재 프로세스 의사 핸들로 토큰을 연다. 아래에서 닫는다.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(os)?;
        let result = (|| {
            let mut len = 0u32;
            // SAFETY: 크기만 묻는다(버퍼 없음, 실패가 정상).
            let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
            if len == 0 {
                return Err(io::Error::other("TokenUser 크기를 얻지 못함"));
            }
            // TOKEN_USER 정렬을 맞추려고 u64 버퍼를 쓴다.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            // SAFETY: buf는 len 바이트 이상이다.
            unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    Some(buf.as_mut_ptr() as *mut c_void),
                    len,
                    &mut len,
                )
            }
            .map_err(os)?;
            // SAFETY: 성공했으므로 buf 앞부분이 TOKEN_USER다.
            let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
            let mut s = PWSTR::null();
            // SAFETY: Sid는 buf 안을 가리킨다. 결과 문자열은 LocalFree로 해제한다.
            unsafe { ConvertSidToStringSidW(user.User.Sid, &mut s) }.map_err(os)?;
            // SAFETY: 위에서 받은 NUL 종료 문자열
            let out = unsafe { s.to_string() }.map_err(|e| io::Error::other(e.to_string()));
            // SAFETY: ConvertSidToStringSidW가 LocalAlloc으로 준 메모리
            unsafe {
                let _ = LocalFree(HLOCAL(s.0 as *mut c_void));
            }
            out
        })();
        // SAFETY: 위에서 연 토큰
        unsafe {
            let _ = CloseHandle(token);
        }
        result
    }

    /// `CreateNamedPipeW`에 넘길 보안 속성. 파이프를 만든 뒤 버려도 된다(설명자는 복사된다).
    pub struct PipeSecurity {
        sd: PSECURITY_DESCRIPTOR,
        sa: SECURITY_ATTRIBUTES,
    }

    // SAFETY: 설명자는 이 값만 가진 LocalAlloc 메모리이고, 만든 스레드와 무관하게 읽기만 한다.
    unsafe impl Send for PipeSecurity {}

    impl PipeSecurity {
        pub fn current_user_only() -> io::Result<Self> {
            let sddl = HSTRING::from(super::user_only_sddl(&current_user_sid()?));
            let mut sd = PSECURITY_DESCRIPTOR::default();
            // SAFETY: sddl은 NUL 종료 문자열, sd는 출력 버퍼. Drop에서 LocalFree.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(sddl.as_ptr()),
                    SDDL_REVISION_1,
                    &mut sd,
                    None,
                )
            }
            .map_err(os)?;
            Ok(Self {
                sd,
                sa: SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: sd.0,
                    bInheritHandle: false.into(),
                },
            })
        }

        /// tokio `create_with_security_attributes_raw`에 넘길 포인터 (self가 살아 있는 동안만 유효)
        pub fn as_raw(&mut self) -> *mut c_void {
            &mut self.sa as *mut SECURITY_ATTRIBUTES as *mut c_void
        }
    }

    impl Drop for PipeSecurity {
        fn drop(&mut self) {
            if !self.sd.0.is_null() {
                // SAFETY: ConvertStringSecurityDescriptorToSecurityDescriptorW가 준 메모리
                unsafe {
                    let _ = LocalFree(HLOCAL(self.sd.0));
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn current_user_sid_and_descriptor() {
            let sid = current_user_sid().unwrap();
            assert!(sid.starts_with("S-1-"), "{sid}");
            let mut s = PipeSecurity::current_user_only().unwrap();
            assert!(!s.as_raw().is_null());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sddl_shape() {
        assert_eq!(
            user_only_sddl("S-1-5-21-1-2-3-1001"),
            "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)"
        );
    }
}
