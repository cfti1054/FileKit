//! 항목 식별 스레드 (STA).
//!
//! Shell COM(`IShellWindows`, `IFolderView2`)은 STA가 필요하고 훅 콜백을 막으면 안 되므로 전용 스레드에서
//! `CoInitializeEx(COINIT_APARTMENTTHREADED)` 후 요청을 하나씩 처리한다.
//!
//! 식별 순서 (spec 4장 3~5단계):
//! 1. `IUIAutomation::ElementFromPoint` → 부모로 올라가며 `ListItem`을 찾는다(`find_list_item`).
//!    없으면 `NotAnItem` (조정자가 삼킨 클릭을 `SendInput`으로 다시 보낸다).
//! 2. 최상위 창에 맞는 `IFolderView2`:
//!    - 탐색기: `IShellWindows`에서 HWND가 같은 항목들 → `QueryService(SID_STopLevelBrowser)` →
//!      `IShellBrowser`. Windows 11 탭이면 같은 HWND 항목이 여러 개라 `IShellBrowser::GetWindow`
//!      (`ShellTabWindowClass`)가 클릭한 탭과 같은 것을 고른다(`pick_active_tab`).
//!    - 바탕화면: `IShellWindows::FindWindowSW(SWC_DESKTOP)`.
//! 3. 클릭한 항목: UIA 이름(표시 이름)을 선택 항목 → 폴더 파일 목록 → 보기 항목 순으로 실제 경로에 맞춘다
//!    (`resolve_clicked`). 경로는 `SIGDN_FILESYSPATH`·실제 파일 이름에서 얻고, 표시 이름으로 경로를 만들지 않는다.
//! 4. `classify::select_targets`로 F-03 규칙 적용. 폴더를 클릭했으면 `NotAnItem`(Windows 11 새 탭 열기 보존).
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
    /// 파일 항목이 아님 (빈 공간, 탐색 창, 머리글, 폴더) → 삼킨 클릭을 다시 보내 원래 동작을 살린다
    NotAnItem,
    /// 식별 실패 (UIPI, COM 오류, 이름 판정 불가) → 다시 보내고 로그만 남긴다
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
    /// `IShellBrowser::GetWindow` (Windows 11은 탭마다 다른 `ShellTabWindowClass`)
    pub tab_hwnd: isize,
    /// `IsWindowVisible(tab_hwnd)`
    pub visible: bool,
}

/// 활성 탭을 고른다.
///
/// 1. 클릭 지점의 `ShellTabWindowClass` 조상(`clicked_tab`, 없으면 0)과 같은 후보
/// 2. 보이는 후보가 하나면 그것
/// 3. 후보가 하나뿐이면(Windows 10) 그것
/// 4. 그 밖은 판정 불가
pub fn pick_active_tab(candidates: &[TabCandidate], clicked_tab: isize) -> Option<usize> {
    if clicked_tab != 0 {
        if let Some(i) = candidates.iter().position(|c| c.tab_hwnd == clicked_tab) {
            return Some(i);
        }
    }
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

/// UIA 컨트롤 형식 ID (`UIA_*ControlTypeId`)
pub const UIA_LIST_ITEM: i32 = 50007;
pub const UIA_LIST: i32 = 50008;
pub const UIA_WINDOW: i32 = 50032;
pub const UIA_PANE: i32 = 50033;
/// UIA 부모를 따라 올라가는 최대 깊이 (항목 안의 이름 칸·편집 칸 → 항목)
pub const UIA_MAX_DEPTH: usize = 6;

/// 커서 아래 요소부터 부모 쪽으로 읽은 컨트롤 형식 목록에서 `ListItem`의 위치를 찾는다.
/// 목록(`List`)·창·창 틀에 닿으면 항목 밖(빈 공간, 머리글, 탐색 창)이므로 멈춘다.
pub fn find_list_item(chain: &[i32]) -> Option<usize> {
    for (i, &t) in chain.iter().enumerate().take(UIA_MAX_DEPTH) {
        match t {
            UIA_LIST_ITEM => return Some(i),
            UIA_LIST | UIA_WINDOW | UIA_PANE => return None,
            _ => {}
        }
    }
    None
}

/// 확장자 숨김 설정과 무관하게 항상 확장자를 숨기는 형식 (`NeverShowExt`)
const ALWAYS_HIDDEN_EXT: [&str; 4] = ["lnk", "url", "pif", "appref-ms"];

/// UIA 이름에 섞일 수 있는 방향 표시 문자를 뺀다.
fn clean_display_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\0'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// 표시 이름(`name`)이 될 수 있는 후보의 위치들.
///
/// - 파일 이름 전체가 같으면 맞음
/// - 확장자를 숨기는 설정(`hide_ext`)이거나 항상 숨기는 형식(.lnk 등)이면 확장자를 뺀 이름이 같아도 맞음
///
/// 대소문자는 가리지 않는다(Windows 파일 이름 규칙).
pub fn match_display_name(name: &str, candidates: &[PathBuf], hide_ext: bool) -> Vec<usize> {
    let want = clean_display_name(name).to_lowercase();
    if want.is_empty() {
        return vec![];
    }
    candidates
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            let Some(file_name) = p.file_name().map(|s| s.to_string_lossy().to_lowercase()) else {
                return false;
            };
            if file_name == want {
                return true;
            }
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if ext.is_empty() || !(hide_ext || ALWAYS_HIDDEN_EXT.contains(&ext.as_str())) {
                return false;
            }
            p.file_stem()
                .is_some_and(|s| s.to_string_lossy().to_lowercase() == want)
        })
        .map(|(i, _)| i)
        .collect()
}

/// 클릭한 항목 판정 결과
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Found(PathBuf),
    /// 같은 표시 이름이 여러 개 (확장자 숨김으로 `a.png`·`a.jpg`가 둘 다 `a`)
    Ambiguous(usize),
    NotFound,
}

/// UIA 이름으로 클릭한 항목을 정한다.
///
/// 선택 항목에 맞는 것이 있으면 그것(여러 개여도 F-03에 따라 선택 전체가 대상이므로 첫 번째를 primary로).
/// 없으면 `others`(폴더 파일 목록 또는 보기 항목)에서 하나로 정해질 때만 고른다.
pub fn resolve_clicked(
    name: &str,
    hide_ext: bool,
    selection: &[PathBuf],
    others: &[PathBuf],
) -> Resolution {
    if let Some(&i) = match_display_name(name, selection, hide_ext).first() {
        return Resolution::Found(selection[i].clone());
    }
    let mut hits: Vec<&PathBuf> = match_display_name(name, others, hide_ext)
        .into_iter()
        .map(|i| &others[i])
        .collect();
    hits.dedup_by(|a, b| {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    });
    match hits.as_slice() {
        [] => Resolution::NotFound,
        [one] => Resolution::Found((*one).clone()),
        many => Resolution::Ambiguous(many.len()),
    }
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, Sender};
    use std::thread::JoinHandle;
    use std::time::Instant;

    use windows::core::{w, Interface, BSTR, PWSTR, VARIANT};
    use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, POINT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IDispatch,
        IServiceProvider, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationElement, IUIAutomationTreeWalker,
    };
    use windows::Win32::UI::Shell::{
        FOLDERID_PublicDesktop, IFolderView2, IPersistFolder2, IShellBrowser, IShellItemArray,
        IShellWindows, IWebBrowserApp, SHGetKnownFolderPath, SHGetNameFromIDList,
        SID_STopLevelBrowser, ShellWindows, CSIDL_DESKTOP, KF_FLAG_DEFAULT, SIGDN_FILESYSPATH,
        SVGIO_ALLVIEW, SWC_DESKTOP, SWFO_NEEDDISPATCH,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetParent, IsWindowVisible, WindowFromPoint,
    };

    use super::{
        find_list_item, resolve_clicked, Identified, IdentifyOutcome, IdentifyRequest, Resolution,
        TabCandidate, UIA_MAX_DEPTH,
    };
    use crate::trigger::classify::{select_targets, HitKind};

    /// UIA 호출 한 번의 상한. 멈춘 탐색기 때문에 휠 클릭이 오래 묶이지 않게 한다.
    const UIA_TIMEOUT_MS: u32 = 400;
    /// 폴더 경로가 없는 보기(홈, 라이브러리, 검색 결과)에서 훑을 항목 수 상한 (항목마다 RPC 한 번)
    const VIEW_SCAN_LIMIT: u32 = 3000;

    fn com_err(what: &str, e: windows::core::Error) -> String {
        format!("{what}: {e}")
    }

    /// STA 스레드 하나가 쥐는 UIA 객체
    pub struct Identifier {
        uia: IUIAutomation,
        walker: IUIAutomationTreeWalker,
    }

    impl Identifier {
        /// STA로 초기화한 스레드에서만 만든다.
        pub fn new() -> windows::core::Result<Self> {
            // SAFETY: COM이 초기화된 스레드에서 부른다.
            let uia: IUIAutomation =
                unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)? };
            if let Ok(uia2) = uia.cast::<IUIAutomation2>() {
                // SAFETY: 단순 설정 호출
                unsafe {
                    let _ = uia2.SetConnectionTimeout(UIA_TIMEOUT_MS);
                    let _ = uia2.SetTransactionTimeout(UIA_TIMEOUT_MS);
                }
            }
            // SAFETY: 위와 같다.
            let walker = unsafe { uia.ControlViewWalker()? };
            // 첫 ElementFromPoint는 UIA 초기화로 느리다. 시작할 때 한 번 불러 둔다(100ms 목표).
            // SAFETY: 결과는 버린다.
            let warm = Instant::now();
            let _ = unsafe { uia.ElementFromPoint(POINT { x: 0, y: 0 }) };
            tracing::debug!(ms = warm.elapsed().as_secs_f64() * 1000.0, "UIA 준비");
            Ok(Self { uia, walker })
        }

        /// 1단계: 커서 아래 ListItem 요소의 이름. 항목이 아니면 Ok(None).
        fn list_item_name(&self, x: i32, y: i32) -> Result<Option<String>, String> {
            // SAFETY: UIA 호출. 실패는 Result로 온다.
            let mut el: IUIAutomationElement = unsafe { self.uia.ElementFromPoint(POINT { x, y }) }
                .map_err(|e| com_err("ElementFromPoint", e))?;
            let mut chain = Vec::with_capacity(UIA_MAX_DEPTH);
            let mut elems = Vec::with_capacity(UIA_MAX_DEPTH);
            for _ in 0..UIA_MAX_DEPTH {
                // SAFETY: 위와 같다.
                let t = unsafe { el.CurrentControlType() }
                    .map_err(|e| com_err("CurrentControlType", e))?;
                chain.push(t.0);
                elems.push(el.clone());
                if matches!(
                    t.0,
                    super::UIA_LIST_ITEM | super::UIA_LIST | super::UIA_WINDOW | super::UIA_PANE
                ) {
                    break;
                }
                // SAFETY: 위와 같다. 루트에 닿으면 Err.
                match unsafe { self.walker.GetParentElement(&el) } {
                    Ok(p) => el = p,
                    Err(_) => break,
                }
            }
            let Some(i) = find_list_item(&chain) else {
                return Ok(None);
            };
            // SAFETY: 위와 같다.
            let name: BSTR =
                unsafe { elems[i].CurrentName() }.map_err(|e| com_err("CurrentName", e))?;
            Ok(Some(name.to_string()))
        }

        /// 요청 하나를 식별한다.
        pub fn identify(&self, req: &IdentifyRequest) -> IdentifyOutcome {
            let started = Instant::now();
            let out = self.identify_inner(req);
            tracing::debug!(
                identify_ms = started.elapsed().as_secs_f64() * 1000.0,
                since_click_ms = req.at.elapsed().as_secs_f64() * 1000.0,
                "항목 식별"
            );
            out
        }

        fn identify_inner(&self, req: &IdentifyRequest) -> IdentifyOutcome {
            let name = match self.list_item_name(req.x, req.y) {
                Ok(Some(n)) => n,
                Ok(None) => return IdentifyOutcome::NotAnItem,
                Err(e) => return IdentifyOutcome::Failed(e),
            };
            let view = match folder_view(req) {
                Ok(v) => v,
                Err(e) => return IdentifyOutcome::Failed(e),
            };
            let folder = view_folder_path(&view);
            let selection = selection_paths(&view);
            let hide_ext = hide_file_ext();

            let mut resolution = resolve_clicked(&name, hide_ext, &selection, &[]);
            if resolution == Resolution::NotFound {
                let mut others = Vec::new();
                if let Some(f) = &folder {
                    others.extend(dir_entries(f));
                }
                if req.hit == HitKind::Desktop {
                    if let Some(p) = known_folder(&FOLDERID_PublicDesktop) {
                        others.extend(dir_entries(&p));
                    }
                }
                if folder.is_none() {
                    others.extend(view_item_paths(&view));
                }
                resolution = resolve_clicked(&name, hide_ext, &selection, &others);
            }
            let clicked = match resolution {
                Resolution::Found(p) => p,
                Resolution::Ambiguous(n) => {
                    return IdentifyOutcome::Failed(format!(
                        "표시 이름이 같은 항목이 {n}개라 클릭한 파일을 정하지 못함"
                    ))
                }
                Resolution::NotFound => {
                    return IdentifyOutcome::Failed("표시 이름에 맞는 파일 없음".into())
                }
            };
            if clicked.is_dir() {
                // 폴더 휠 클릭 = Windows 11 "새 탭에서 열기". 원래 동작을 살린다.
                return IdentifyOutcome::NotAnItem;
            }
            let files: Vec<PathBuf> = select_targets(&clicked, &selection)
                .into_iter()
                .filter(|p| !p.is_dir())
                .collect();
            IdentifyOutcome::Files(Identified {
                files,
                primary: clicked,
            })
        }
    }

    fn pwstr_take(p: PWSTR) -> Option<String> {
        if p.is_null() {
            return None;
        }
        // SAFETY: 셸이 CoTaskMemAlloc으로 준 NUL 종료 문자열. 읽은 뒤 해제한다.
        let s = unsafe { p.to_string() }.ok();
        // SAFETY: 위와 같다.
        unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
        s
    }

    fn class_is(hwnd: HWND, expected: &str) -> bool {
        let mut buf = [0u16; 256];
        // SAFETY: buf는 유효한 쓰기 버퍼
        let n = unsafe { GetClassNameW(hwnd, &mut buf) };
        let n = usize::try_from(n).unwrap_or(0).min(buf.len());
        String::from_utf16_lossy(&buf[..n]) == expected
    }

    /// 클릭 지점의 `ShellTabWindowClass` 조상 (Windows 11 탭). 없으면 0.
    fn clicked_tab(x: i32, y: i32) -> isize {
        // SAFETY: 단순 조회
        let mut h = unsafe { WindowFromPoint(POINT { x, y }) };
        for _ in 0..16 {
            if h.is_invalid() {
                break;
            }
            if class_is(h, "ShellTabWindowClass") {
                return h.0 as isize;
            }
            // SAFETY: 단순 조회. 최상위면 Err.
            h = match unsafe { GetParent(h) } {
                Ok(p) => p,
                Err(_) => break,
            };
        }
        0
    }

    fn browser_of(disp: &IDispatch) -> windows::core::Result<IShellBrowser> {
        let sp: IServiceProvider = disp.cast()?;
        // SAFETY: COM 호출
        unsafe { sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) }
    }

    fn view_of(browser: &IShellBrowser) -> windows::core::Result<IFolderView2> {
        // SAFETY: COM 호출
        unsafe { browser.QueryActiveShellView()?.cast() }
    }

    /// 2단계: 클릭한 창(탭)의 `IFolderView2`
    fn folder_view(req: &IdentifyRequest) -> Result<IFolderView2, String> {
        // SAFETY: COM 호출. ShellWindows는 explorer.exe의 로컬 서버다.
        let windows: IShellWindows = unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) }
            .map_err(|e| com_err("ShellWindows", e))?;

        if req.hit == HitKind::Desktop {
            let loc = VARIANT::from(CSIDL_DESKTOP as i32);
            let empty = VARIANT::default();
            let mut hwnd = 0i32;
            // SAFETY: 인자는 유효한 VARIANT와 출력 버퍼
            let disp = unsafe {
                windows.FindWindowSW(&loc, &empty, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)
            }
            .map_err(|e| com_err("FindWindowSW(바탕화면)", e))?;
            let browser = browser_of(&disp).map_err(|e| com_err("바탕화면 IShellBrowser", e))?;
            return view_of(&browser).map_err(|e| com_err("바탕화면 IFolderView2", e));
        }

        let tab = clicked_tab(req.x, req.y);
        // SAFETY: COM 호출
        let count = unsafe { windows.Count() }.map_err(|e| com_err("ShellWindows.Count", e))?;
        let mut browsers = Vec::new();
        let mut candidates = Vec::new();
        for i in 0..count {
            // SAFETY: COM 호출. 닫히는 중인 창은 Err일 수 있어 건너뛴다.
            let Ok(disp) = (unsafe { windows.Item(&VARIANT::from(i)) }) else {
                continue;
            };
            let Ok(app) = disp.cast::<IWebBrowserApp>() else {
                continue;
            };
            // SAFETY: COM 호출
            let Ok(h) = (unsafe { app.HWND() }) else {
                continue;
            };
            if h.0 != req.root_hwnd {
                continue;
            }
            let Ok(browser) = browser_of(&disp) else {
                continue;
            };
            // SAFETY: COM 호출. Windows 11에서는 탭의 ShellTabWindowClass HWND.
            let tab_hwnd = unsafe { browser.GetWindow() }.unwrap_or_default();
            // SAFETY: 단순 조회
            let visible = !tab_hwnd.is_invalid() && unsafe { IsWindowVisible(tab_hwnd) }.as_bool();
            candidates.push(TabCandidate {
                tab_hwnd: tab_hwnd.0 as isize,
                visible,
            });
            browsers.push(browser);
        }
        let pick = super::pick_active_tab(&candidates, tab).ok_or_else(|| {
            format!(
                "탐색기 창(탭)을 정하지 못함: 후보 {}개, 클릭 탭 {tab:#x}",
                candidates.len()
            )
        })?;
        view_of(&browsers[pick]).map_err(|e| com_err("IFolderView2", e))
    }

    /// 보기의 폴더 파일 시스템 경로 (홈·라이브러리·검색 결과 등은 None)
    fn view_folder_path(view: &IFolderView2) -> Option<PathBuf> {
        // SAFETY: COM 호출. pidl은 CoTaskMem으로 받아 해제한다.
        unsafe {
            let pf: IPersistFolder2 = view.GetFolder().ok()?;
            let pidl = pf.GetCurFolder().ok()?;
            let name = SHGetNameFromIDList(pidl, SIGDN_FILESYSPATH).ok();
            CoTaskMemFree(Some(pidl as *const c_void));
            name.and_then(pwstr_take).map(PathBuf::from)
        }
    }

    fn array_paths(arr: &IShellItemArray, limit: u32) -> Vec<PathBuf> {
        // SAFETY: COM 호출
        let n = unsafe { arr.GetCount() }.unwrap_or(0).min(limit);
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            // SAFETY: COM 호출
            let path = unsafe { arr.GetItemAt(i) }
                .and_then(|item| unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) })
                .ok()
                .and_then(pwstr_take);
            if let Some(p) = path {
                out.push(PathBuf::from(p));
            }
        }
        out
    }

    /// 선택 항목의 파일 시스템 경로 (가상 항목은 빠진다)
    fn selection_paths(view: &IFolderView2) -> Vec<PathBuf> {
        // SAFETY: COM 호출. 선택이 없으면 Err.
        match unsafe { view.GetSelection(false) } {
            Ok(arr) => array_paths(&arr, u32::MAX),
            Err(_) => vec![],
        }
    }

    /// 보기의 모든 항목 경로 (폴더 경로가 없을 때만, 상한 있음)
    fn view_item_paths(view: &IFolderView2) -> Vec<PathBuf> {
        // SAFETY: COM 호출
        match unsafe { view.Items::<IShellItemArray>(SVGIO_ALLVIEW) } {
            Ok(arr) => array_paths(&arr, VIEW_SCAN_LIMIT),
            Err(_) => vec![],
        }
    }

    fn dir_entries(dir: &Path) -> Vec<PathBuf> {
        match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).collect(),
            Err(e) => {
                tracing::debug!(dir = %dir.display(), "폴더 읽기 실패: {e}");
                vec![]
            }
        }
    }

    fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
        // SAFETY: 결과 문자열은 pwstr_take가 해제한다.
        unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }
            .ok()
            .and_then(pwstr_take)
            .map(PathBuf::from)
    }

    /// 탐색기 "알려진 파일 형식의 확장명 숨기기" (읽기 전용, 값이 없으면 Windows 기본값 숨김)
    fn hide_file_ext() -> bool {
        let mut value: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: value/size는 유효한 출력 버퍼다. 읽기만 한다.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced"),
                w!("HideFileExt"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut u32 as *mut c_void),
                Some(&mut size),
            )
        };
        r != ERROR_SUCCESS || value != 0
    }

    /// 식별 스레드 핸들. Drop하면 요청 채널을 닫고 join한다.
    pub struct ExplorerThread {
        tx: Option<Sender<IdentifyRequest>>,
        join: Option<JoinHandle<()>>,
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
                let identifier = if com_ok {
                    match Identifier::new() {
                        Ok(i) => Some(i),
                        Err(e) => {
                            tracing::warn!("UI Automation을 만들지 못함: {e}");
                            None
                        }
                    }
                } else {
                    None
                };
                while let Ok(req) = rx.recv() {
                    let outcome = match &identifier {
                        Some(i) => i.identify(&req),
                        None => IdentifyOutcome::Failed("COM/UIA 초기화 실패".into()),
                    };
                    on_result(req, outcome);
                }
                drop(identifier);
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
        use std::time::Duration;

        /// 화면 밖 좌표: 항목이 없으므로 NotAnItem 또는 Failed(UIA 없음) — 어느 쪽이든 다시 보내기
        #[test]
        fn thread_round_trip_off_screen_needs_replay() {
            let (otx, orx) = mpsc::channel();
            let t = start(move |req, out| {
                let _ = otx.send((req, out));
            })
            .unwrap();
            assert!(t.send(IdentifyRequest {
                x: -32000,
                y: -32000,
                hit: HitKind::ExplorerFileList,
                root_hwnd: 0,
                at: Instant::now(),
            }));
            let (req, out) = orx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!((req.x, req.y), (-32000, -32000));
            assert!(out.needs_replay(), "{out:?}");
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

    fn t(h: isize, v: bool) -> TabCandidate {
        TabCandidate {
            tab_hwnd: h,
            visible: v,
        }
    }

    #[test]
    fn active_tab_selection() {
        // Windows 10: 탭 없음, 후보 하나
        assert_eq!(pick_active_tab(&[t(0, false)], 0), Some(0));
        // Windows 11: 탭 세 개 중 보이는 것 하나
        assert_eq!(
            pick_active_tab(&[t(1, false), t(2, true), t(3, false)], 0),
            Some(1)
        );
        // 판정 불가
        assert_eq!(pick_active_tab(&[t(1, false), t(2, false)], 0), None);
        assert_eq!(pick_active_tab(&[t(1, true), t(2, true)], 0), None);
        assert_eq!(pick_active_tab(&[], 0), None);
    }

    /// Windows 11 회귀: 클릭한 탭 HWND가 보이는 탭 판정보다 앞선다
    #[test]
    fn active_tab_prefers_clicked_tab() {
        let c = [t(10, true), t(20, true), t(30, false)];
        assert_eq!(pick_active_tab(&c, 20), Some(1));
        assert_eq!(pick_active_tab(&c, 30), Some(2));
        // 클릭 탭이 후보에 없으면 보이는 탭 규칙으로 (여기서는 둘이라 판정 불가)
        assert_eq!(pick_active_tab(&c, 99), None);
        assert_eq!(pick_active_tab(&[t(10, false), t(20, true)], 99), Some(1));
    }

    #[test]
    fn list_item_search() {
        // 항목 자체
        assert_eq!(find_list_item(&[UIA_LIST_ITEM]), Some(0));
        // 자세히 보기의 이름 칸(Edit 50004) → 항목
        assert_eq!(find_list_item(&[50004, UIA_LIST_ITEM]), Some(1));
        // 텍스트(50020) → 이미지(50006) → 항목
        assert_eq!(find_list_item(&[50020, 50006, UIA_LIST_ITEM]), Some(2));
        // 빈 공간: 목록
        assert_eq!(find_list_item(&[UIA_LIST]), None);
        // 머리글(50034 Header → 50035 HeaderItem) → 창 틀
        assert_eq!(find_list_item(&[50035, 50034, UIA_PANE]), None);
        // 탐색 창 트리 항목(50024 TreeItem) → 트리(50023) → 창 틀
        assert_eq!(find_list_item(&[50024, 50023, UIA_PANE]), None);
        assert_eq!(find_list_item(&[]), None);
        // 너무 깊으면 찾지 않는다
        let mut deep = vec![50020; UIA_MAX_DEPTH];
        deep.push(UIA_LIST_ITEM);
        assert_eq!(find_list_item(&deep), None);
    }

    fn paths(v: &[&str]) -> Vec<PathBuf> {
        v.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn display_name_matching() {
        let c = paths(&[
            r"C:\v\발표 녹화.mov",
            r"C:\v\a.png",
            r"C:\v\a.jpg",
            r"C:\v\메모",
            r"C:\v\바로 가기.lnk",
            r"C:\v\.gitignore",
            r"C:\v\v1.2",
        ]);
        // 확장자 보이기
        assert_eq!(match_display_name("발표 녹화.mov", &c, false), vec![0]);
        assert_eq!(
            match_display_name("발표 녹화", &c, false),
            Vec::<usize>::new()
        );
        // 확장자 숨기기
        assert_eq!(match_display_name("발표 녹화", &c, true), vec![0]);
        assert_eq!(match_display_name("a", &c, true), vec![1, 2]);
        // .lnk는 설정과 무관하게 확장자를 숨긴다
        assert_eq!(match_display_name("바로 가기", &c, false), vec![4]);
        // 대소문자·방향 표시 문자 무시
        assert_eq!(match_display_name("A.PNG", &c, false), vec![1]);
        assert_eq!(
            match_display_name("\u{200E}메모\u{200F}", &c, false),
            vec![3]
        );
        assert_eq!(match_display_name(".gitignore", &c, true), vec![5]);
        assert_eq!(match_display_name("", &c, true), Vec::<usize>::new());
    }

    #[test]
    fn resolve_prefers_selection() {
        let sel = paths(&[r"C:\v\a.png"]);
        let others = paths(&[r"C:\v\a.png", r"C:\v\a.jpg"]);
        // 숨김으로 둘 다 "a"지만 선택에 있는 것
        assert_eq!(
            resolve_clicked("a", true, &sel, &others),
            Resolution::Found(PathBuf::from(r"C:\v\a.png"))
        );
        // 선택에 없으면 판정 불가
        assert_eq!(
            resolve_clicked("a", true, &[], &others),
            Resolution::Ambiguous(2)
        );
        // 하나로 정해지면 찾음
        assert_eq!(
            resolve_clicked("a.jpg", true, &[], &others),
            Resolution::Found(PathBuf::from(r"C:\v\a.jpg"))
        );
        assert_eq!(
            resolve_clicked("없음", true, &sel, &others),
            Resolution::NotFound
        );
    }

    #[test]
    fn resolve_dedups_same_path_from_two_sources() {
        // 바탕화면: 폴더 목록과 보기 항목에 같은 파일이 두 번 들어와도 하나로 본다
        let others = paths(&[r"C:\Users\u\Desktop\b.png", r"c:\users\u\desktop\B.png"]);
        assert_eq!(
            resolve_clicked("b.png", false, &[], &others),
            Resolution::Found(PathBuf::from(r"C:\Users\u\Desktop\b.png"))
        );
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
