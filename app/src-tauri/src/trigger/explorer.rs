//! 항목 식별 스레드 (STA).
//!
//! Shell COM(`IShellWindows`, `IFolderView2`)은 STA가 필요하고 훅 콜백을 막으면 안 되므로 전용 스레드에서
//! `CoInitializeEx(COINIT_APARTMENTTHREADED)` 후 요청을 하나씩 처리한다.
//!
//! 식별 순서 (spec 4장 3~5단계, **TODO(F-01)**):
//! 1. `IUIAutomation::ElementFromPoint` → `ControlType`이 `ListItem`이 아니면 `NotAnItem`
//!    (조정자가 삼킨 클릭을 `SendInput`으로 down·up 다시 보낸다. `dwExtraInfo = OWN_INPUT_MARK`)
//! 2. `IShellWindows`에서 HWND가 같은 창 → `IServiceProvider::QueryService(SID_STopLevelBrowser)` →
//!    `IShellBrowser::QueryActiveShellView` → `IFolderView2`
//!    - Windows 11 탭: 같은 HWND 항목이 여러 개면 보이는 `ShellTabWindowClass`와 대응하는 탭(`pick_active_tab`)
//!    - 바탕화면: `IShellWindows::FindWindowSW(SWC_DESKTOP)`
//! 3. 클릭한 항목과 선택 항목의 `IShellItem::GetDisplayName(SIGDN_FILESYSPATH)` (확장자 숨김 때문에
//!    표시 이름으로 경로를 만들지 않는다). 파싱 이름만 있으면 `join_folder_item`.
//! 4. `classify::select_targets`로 F-03 규칙 적용.
//!
//! UIPI(관리자 권한 탐색기)로 실패하면 오류를 띄우지 않고 `Failed`로 로그만 남긴다.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::classify::HitKind;

/// 훅 → 식별 스레드 요청
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifyRequest {
    /// 화면 물리 px
    pub x: i32,
    pub y: i32,
    pub hit: HitKind,
    pub root_hwnd: isize,
    /// 휠 버튼을 누른 시각 (100ms 목표 측정 기준점)
    pub at: Instant,
}

/// 식별 결과: 변환 대상 파일
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identified {
    /// F-03 규칙을 적용한 대상 (선택 전체 또는 클릭한 파일 하나)
    pub files: Vec<PathBuf>,
    /// 클릭한 파일 (`query_formats`의 primary)
    pub primary: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifyOutcome {
    Files(Identified),
    /// 파일 항목이 아님 (빈 공간, 탐색 창, 머리글) → 삼킨 클릭을 다시 보내 원래 동작을 살린다
    NotAnItem,
    /// 식별 실패 (UIPI, COM 오류, 미구현) → 다시 보내고 로그만 남긴다
    Failed(String),
}

impl IdentifyOutcome {
    /// 삼킨 클릭을 `SendInput`으로 다시 보내야 하는지
    pub fn needs_replay(&self) -> bool {
        !matches!(self, IdentifyOutcome::Files(_))
    }
}

/// 폴더 경로와 항목의 파싱 이름을 합친다.
///
/// - 파싱 이름이 이미 절대 경로(`C:\…`, `\\server\…`)면 그대로 쓴다.
/// - 셸 네임스페이스 항목(`::{GUID}`), 빈 이름, 경로 구분자·`..`가 들어간 이름은 파일이 아니므로 `None`.
pub fn join_folder_item(folder: Option<&Path>, parsing_name: &str) -> Option<PathBuf> {
    let name = parsing_name.trim_end_matches('\0');
    if name.is_empty() || name.starts_with("::") {
        return None;
    }
    if is_absolute_windows_path(name) {
        return Some(PathBuf::from(name));
    }
    if name.contains(['\\', '/']) || name == "." || name == ".." {
        return None;
    }
    folder.map(|f| f.join(name))
}

/// `C:\…` 또는 `\\server\share\…` (플랫폼과 무관하게 Windows 규칙으로 판정)
pub fn is_absolute_windows_path(s: &str) -> bool {
    let b = s.as_bytes();
    let drive = b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/');
    let unc = s.starts_with(r"\\") && s.len() > 2;
    drive || unc
}

/// Windows 11 탐색기 탭 후보: 같은 `CabinetWClass` HWND에 묶인 `IShellWindows` 항목 하나
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabCandidate {
    /// 대응하는 `ShellTabWindowClass` 창 (Windows 10이면 0)
    pub tab_hwnd: isize,
    /// `IsWindowVisible(tab_hwnd)`
    pub visible: bool,
}

/// 활성 탭을 고른다. 보이는 탭이 하나면 그것, 후보가 하나뿐이면(Windows 10) 그것, 그 밖은 판정 불가.
pub fn pick_active_tab(candidates: &[TabCandidate]) -> Option<usize> {
    let mut visible = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.visible)
        .map(|(i, _)| i);
    match (visible.next(), visible.next()) {
        (Some(i), None) => Some(i),
        (Some(_), Some(_)) => None,
        (None, _) if candidates.len() == 1 => Some(0),
        (None, _) => None,
    }
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use std::io;
    use std::sync::mpsc::{self, Sender};
    use std::thread::JoinHandle;

    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    use super::{IdentifyOutcome, IdentifyRequest};

    /// 식별 스레드 핸들. Drop하면 요청 채널을 닫고 join한다.
    pub struct ExplorerThread {
        tx: Option<Sender<IdentifyRequest>>,
        join: Option<JoinHandle<()>>,
    }

    /// 요청 하나를 식별한다. STA 스레드에서만 부른다.
    pub fn identify(req: &IdentifyRequest) -> IdentifyOutcome {
        // TODO(F-01): 모듈 문서의 1~4단계 (UIA ElementFromPoint, IShellWindows, IFolderView2,
        //             Windows 11 탭 선택, SIGDN_FILESYSPATH, select_targets).
        IdentifyOutcome::Failed(format!(
            "항목 식별 미구현 ({:?}, {}, {})",
            req.hit, req.x, req.y
        ))
    }

    /// 식별 스레드를 시작한다. 결과는 `on_result`로 (식별 스레드에서) 넘긴다.
    pub fn start<F>(mut on_result: F) -> io::Result<ExplorerThread>
    where
        F: FnMut(IdentifyRequest, IdentifyOutcome) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<IdentifyRequest>();
        let join = std::thread::Builder::new()
            .name("convtool-identify".into())
            .spawn(move || {
                // SAFETY: 이 스레드를 STA로 초기화한다. 성공했을 때만 끝에서 CoUninitialize.
                let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                let com_ok = hr.is_ok();
                if !com_ok {
                    tracing::warn!("식별 스레드 COM 초기화 실패: {hr:?}");
                }
                while let Ok(req) = rx.recv() {
                    let outcome = if com_ok {
                        identify(&req)
                    } else {
                        IdentifyOutcome::Failed("COM 초기화 실패".into())
                    };
                    on_result(req, outcome);
                }
                if com_ok {
                    // SAFETY: 위 CoInitializeEx와 짝.
                    unsafe { CoUninitialize() };
                }
            })?;
        Ok(ExplorerThread {
            tx: Some(tx),
            join: Some(join),
        })
    }

    impl ExplorerThread {
        /// 요청을 넘긴다. 스레드가 끝났으면 false.
        pub fn send(&self, req: IdentifyRequest) -> bool {
            self.tx.as_ref().is_some_and(|tx| tx.send(req).is_ok())
        }
    }

    impl Drop for ExplorerThread {
        fn drop(&mut self) {
            self.tx.take();
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::trigger::classify::HitKind;
        use std::time::{Duration, Instant};

        #[test]
        fn thread_round_trip_reports_unimplemented() {
            let (otx, orx) = mpsc::channel();
            let t = start(move |req, out| {
                let _ = otx.send((req, out));
            })
            .unwrap();
            assert!(t.send(IdentifyRequest {
                x: 10,
                y: 20,
                hit: HitKind::ExplorerFileList,
                root_hwnd: 0,
                at: Instant::now(),
            }));
            let (req, out) = orx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!((req.x, req.y), (10, 20));
            assert!(matches!(out, IdentifyOutcome::Failed(_)));
            assert!(out.needs_replay());
            drop(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_relative_name() {
        let p = join_folder_item(Some(Path::new(r"C:\영상")), "발표 녹화.mov").unwrap();
        assert_eq!(p, Path::new(r"C:\영상").join("발표 녹화.mov"));
    }

    #[test]
    fn join_keeps_absolute_parsing_name() {
        let p = join_folder_item(Some(Path::new(r"C:\x")), r"D:\문서\보고서.docx").unwrap();
        assert_eq!(p, PathBuf::from(r"D:\문서\보고서.docx"));
        let p = join_folder_item(None, r"\\nas\공유\a.png").unwrap();
        assert_eq!(p, PathBuf::from(r"\\nas\공유\a.png"));
    }

    #[test]
    fn join_rejects_non_files() {
        assert_eq!(
            join_folder_item(
                Some(Path::new(r"C:\x")),
                "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"
            ),
            None
        );
        assert_eq!(join_folder_item(Some(Path::new(r"C:\x")), ""), None);
        assert_eq!(join_folder_item(Some(Path::new(r"C:\x")), ".."), None);
        assert_eq!(join_folder_item(Some(Path::new(r"C:\x")), r"a\b.txt"), None);
        assert_eq!(join_folder_item(None, "a.txt"), None);
    }

    #[test]
    fn join_strips_trailing_nul() {
        let p = join_folder_item(Some(Path::new(r"C:\x")), "a.png\0").unwrap();
        assert_eq!(p, Path::new(r"C:\x").join("a.png"));
    }

    #[test]
    fn absolute_path_detection() {
        assert!(is_absolute_windows_path(r"C:\a"));
        assert!(is_absolute_windows_path("c:/a"));
        assert!(is_absolute_windows_path(r"\\server\share"));
        assert!(!is_absolute_windows_path("C:a"));
        assert!(!is_absolute_windows_path("a.txt"));
        assert!(!is_absolute_windows_path(r"\\"));
    }

    #[test]
    fn active_tab_selection() {
        let t = |h, v| TabCandidate {
            tab_hwnd: h,
            visible: v,
        };
        // Windows 10: 탭 없음, 후보 하나
        assert_eq!(pick_active_tab(&[t(0, false)]), Some(0));
        // Windows 11: 탭 세 개 중 보이는 것 하나
        assert_eq!(
            pick_active_tab(&[t(1, false), t(2, true), t(3, false)]),
            Some(1)
        );
        // 판정 불가
        assert_eq!(pick_active_tab(&[t(1, false), t(2, false)]), None);
        assert_eq!(pick_active_tab(&[t(1, true), t(2, true)]), None);
        assert_eq!(pick_active_tab(&[]), None);
    }

    #[test]
    fn replay_rule() {
        assert!(IdentifyOutcome::NotAnItem.needs_replay());
        assert!(IdentifyOutcome::Failed("x".into()).needs_replay());
        assert!(!IdentifyOutcome::Files(Identified {
            files: vec![],
            primary: PathBuf::new()
        })
        .needs_replay());
    }
}
