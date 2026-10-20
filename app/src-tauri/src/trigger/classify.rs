//! 휠 클릭 트리거의 판정 로직 (순수 함수, Win32 호출 없음).
//!
//! 훅 콜백(`hook.rs`)은 여기 함수만 부른다. 창 클래스 이름 읽기는 콜백이 하고, UIA·COM은 하지 않는다.

use std::path::{Path, PathBuf};

/// 커서 아래 창의 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HitKind {
    /// 탐색기 창(`CabinetWClass`)의 파일 목록(`DirectUIHWND`)
    ExplorerFileList,
    /// 바탕화면 (`Progman` / `WorkerW`)
    Desktop,
    /// 그 밖: 그대로 통과
    Other,
}

impl HitKind {
    pub fn is_candidate(self) -> bool {
        !matches!(self, HitKind::Other)
    }
}

/// `SendInput`으로 다시 보낸 우리 입력의 `dwExtraInfo` 표식 ("CVTL")
pub const OWN_INPUT_MARK: usize = 0x4356_544C;
/// `MSLLHOOKSTRUCT.flags`의 `LLMHF_INJECTED`
pub const LLMHF_INJECTED: u32 = 0x0000_0001;

/// 창 클래스 이름 최대 길이(`WNDCLASS.lpszClassName` 256자). 훅 콜백의 고정 버퍼 크기로 쓴다.
pub const CLASS_NAME_BUF_LEN: usize = 256;

/// ASCII 문자열 리터럴을 컴파일 시간에 UTF-16 배열로 바꾼다. 길이가 다르거나 ASCII가 아니면 컴파일 오류.
const fn ascii_utf16<const N: usize>(s: &str) -> [u16; N] {
    let b = s.as_bytes();
    assert!(b.len() == N);
    let mut out = [0u16; N];
    let mut i = 0;
    while i < N {
        assert!(b[i].is_ascii());
        out[i] = b[i] as u16;
        i += 1;
    }
    out
}

const CABINET_WCLASS: [u16; 13] = ascii_utf16("CabinetWClass");
const DIRECT_UI_HWND: [u16; 12] = ascii_utf16("DirectUIHWND");
const PROGMAN: [u16; 7] = ascii_utf16("Progman");
const WORKER_W: [u16; 7] = ascii_utf16("WorkerW");

/// 창 클래스 이름 비교. `&str`(테스트·일반 코드)과 `&[u16]`(훅 콜백, 할당 없음) 둘 다 받는다.
pub trait ClassName {
    /// 대소문자까지 정확히 같은지 (클래스 이름은 고정 문자열)
    fn is(&self, expected: &[u16]) -> bool;
}

impl ClassName for &[u16] {
    fn is(&self, expected: &[u16]) -> bool {
        *self == expected
    }
}

impl ClassName for &str {
    fn is(&self, expected: &[u16]) -> bool {
        self.encode_utf16().eq(expected.iter().copied())
    }
}

fn classify_impl(root: impl ClassName, hit: impl ClassName) -> HitKind {
    if root.is(&CABINET_WCLASS) {
        if hit.is(&DIRECT_UI_HWND) {
            HitKind::ExplorerFileList
        } else {
            HitKind::Other
        }
    } else if root.is(&PROGMAN) || root.is(&WORKER_W) {
        HitKind::Desktop
    } else {
        HitKind::Other
    }
}

/// 최상위 창 클래스(`GetAncestor(GA_ROOT)`)와 커서 바로 아래 창 클래스(`WindowFromPoint`)로 분류한다.
pub fn classify(root_class: &str, hit_class: &str) -> HitKind {
    classify_impl(root_class, hit_class)
}

/// `classify`의 UTF-16 버전. 훅 콜백이 `GetClassNameW` 버퍼를 그대로 넘긴다(힙 할당 없음).
/// 슬라이스는 NUL을 뺀 길이여야 한다.
pub fn classify_utf16(root_class: &[u16], hit_class: &[u16]) -> HitKind {
    classify_impl(root_class, hit_class)
}

/// 우리가 `SendInput`으로 다시 보낸 입력인지 (무한 반복 방지)
pub fn is_own_injected(flags: u32, extra_info: usize) -> bool {
    flags & LLMHF_INJECTED != 0 && extra_info == OWN_INPUT_MARK
}

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    // Windows 경로는 대소문자를 가리지 않는다.
    a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
}

/// F-03: 클릭한 항목이 선택 집합에 있으면 선택 전체, 아니면 클릭한 항목 하나.
pub fn select_targets(clicked: &Path, selection: &[PathBuf]) -> Vec<PathBuf> {
    if selection.iter().any(|p| same_path(p, clicked)) {
        selection.to_vec()
    } else {
        vec![clicked.to_path_buf()]
    }
}

/// 훅이 보는 마우스 메시지 (필요한 것만)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseMsg {
    MButtonDown,
    MButtonUp,
    Other,
}

/// 훅 콜백이 할 일
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookDecision {
    /// `CallNextHookEx`로 넘긴다
    Pass,
    /// 휠 버튼 누름을 삼키고(`LRESULT(1)`) 식별 스레드에 넘긴다
    SwallowDown(HitKind),
    /// 앞서 삼킨 누름의 짝이 되는 떼기를 삼킨다 (앱에 반쪽 클릭이 가지 않게)
    SwallowUp,
}

/// 휠 버튼 상태를 기억하는 판정기. 훅 스레드 하나에서만 쓴다.
#[derive(Debug, Default)]
pub struct ButtonTracker {
    swallowed_down: bool,
}

impl ButtonTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// `classify_hit`은 휠 버튼 누름일 때만 부른다 (창 클래스 읽기 비용을 아끼려고).
    pub fn decide(
        &mut self,
        msg: MouseMsg,
        flags: u32,
        extra_info: usize,
        paused: bool,
        classify_hit: impl FnOnce() -> HitKind,
    ) -> HookDecision {
        if is_own_injected(flags, extra_info) {
            return HookDecision::Pass;
        }
        match msg {
            MouseMsg::MButtonDown => {
                self.swallowed_down = false;
                if paused {
                    return HookDecision::Pass;
                }
                let hit = classify_hit();
                if hit.is_candidate() {
                    self.swallowed_down = true;
                    HookDecision::SwallowDown(hit)
                } else {
                    HookDecision::Pass
                }
            }
            MouseMsg::MButtonUp => {
                if self.swallowed_down {
                    self.swallowed_down = false;
                    HookDecision::SwallowUp
                } else {
                    HookDecision::Pass
                }
            }
            MouseMsg::Other => HookDecision::Pass,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_explorer_file_list() {
        assert_eq!(
            classify("CabinetWClass", "DirectUIHWND"),
            HitKind::ExplorerFileList
        );
    }

    #[test]
    fn classify_explorer_other_parts_pass() {
        // 탐색 창(트리), 주소 표시줄, 탭 줄 등
        assert_eq!(classify("CabinetWClass", "SysTreeView32"), HitKind::Other);
        assert_eq!(classify("CabinetWClass", "ToolbarWindow32"), HitKind::Other);
        assert_eq!(
            classify(
                "CabinetWClass",
                "Microsoft.UI.Content.DesktopChildSiteBridge"
            ),
            HitKind::Other
        );
    }

    #[test]
    fn classify_desktop() {
        assert_eq!(classify("Progman", "SysListView32"), HitKind::Desktop);
        assert_eq!(classify("WorkerW", "SysListView32"), HitKind::Desktop);
        assert_eq!(classify("Progman", "SHELLDLL_DefView"), HitKind::Desktop);
    }

    #[test]
    fn classify_others_pass() {
        // 브라우저 새 탭 휠 클릭, 파일 대화 상자 등
        assert_eq!(
            classify("Chrome_WidgetWin_1", "Chrome_RenderWidgetHostHWND"),
            HitKind::Other
        );
        assert_eq!(classify("#32770", "DirectUIHWND"), HitKind::Other);
        assert_eq!(classify("", ""), HitKind::Other);
        // 대소문자를 정확히 비교한다 (클래스 이름은 고정 문자열)
        assert_eq!(classify("cabinetwclass", "DirectUIHWND"), HitKind::Other);
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn classify_utf16_matches_str_version() {
        let cases = [
            ("CabinetWClass", "DirectUIHWND"),
            ("CabinetWClass", "SysTreeView32"),
            ("CabinetWClass", "DirectUIHWNDx"),
            ("CabinetWClass", "DirectUIHWN"),
            ("CabinetWClas", "DirectUIHWND"),
            ("CabinetWClassX", "DirectUIHWND"),
            ("Progman", "SysListView32"),
            ("WorkerW", "SHELLDLL_DefView"),
            ("WorkerW", ""),
            ("Progma", "SysListView32"),
            ("progman", "SysListView32"),
            ("#32770", "DirectUIHWND"),
            ("Chrome_WidgetWin_1", "Chrome_RenderWidgetHostHWND"),
            ("탐색기", "DirectUIHWND"),
            ("", ""),
        ];
        for (root, hit) in cases {
            assert_eq!(
                classify_utf16(&w(root), &w(hit)),
                classify(root, hit),
                "{root} / {hit}"
            );
        }
    }

    #[test]
    fn classify_utf16_explicit() {
        assert_eq!(
            classify_utf16(&w("CabinetWClass"), &w("DirectUIHWND")),
            HitKind::ExplorerFileList
        );
        assert_eq!(
            classify_utf16(&w("Progman"), &w("SysListView32")),
            HitKind::Desktop
        );
        assert_eq!(
            classify_utf16(&w("WorkerW"), &w("SysListView32")),
            HitKind::Desktop
        );
        assert_eq!(classify_utf16(&[], &[]), HitKind::Other);
        // 훅이 NUL을 잘라내지 않으면 맞지 않는다 (길이 계산 실수 방지)
        let mut root = w("Progman");
        root.push(0);
        assert_eq!(classify_utf16(&root, &[]), HitKind::Other);
    }

    #[test]
    fn classify_utf16_full_buffer_does_not_match() {
        // GetClassNameW가 버퍼 끝까지 채운(잘린) 이름은 짧은 상수와 같을 수 없다
        let mut root = [0u16; CLASS_NAME_BUF_LEN];
        root[..13].copy_from_slice(&w("CabinetWClass"));
        assert_eq!(classify_utf16(&root, &w("DirectUIHWND")), HitKind::Other);
    }

    #[test]
    fn own_injected_mark() {
        assert!(is_own_injected(LLMHF_INJECTED, OWN_INPUT_MARK));
        assert!(!is_own_injected(0, OWN_INPUT_MARK));
        assert!(!is_own_injected(LLMHF_INJECTED, 0));
        assert!(!is_own_injected(LLMHF_INJECTED | 0x2, 1234));
        assert!(is_own_injected(LLMHF_INJECTED | 0x2, OWN_INPUT_MARK));
    }

    #[test]
    fn select_targets_clicked_in_selection() {
        let sel = vec![
            PathBuf::from(r"C:\영상\a.mov"),
            PathBuf::from(r"C:\영상\발표 녹화.mov"),
        ];
        let t = select_targets(Path::new(r"C:\영상\발표 녹화.mov"), &sel);
        assert_eq!(t, sel);
    }

    #[test]
    fn select_targets_case_insensitive() {
        let sel = vec![
            PathBuf::from(r"C:\Videos\A.MOV"),
            PathBuf::from(r"C:\Videos\b.mov"),
        ];
        let t = select_targets(Path::new(r"c:\videos\a.mov"), &sel);
        assert_eq!(t, sel);
    }

    #[test]
    fn select_targets_clicked_outside_selection() {
        let sel = vec![PathBuf::from(r"C:\a.mov"), PathBuf::from(r"C:\b.mov")];
        let t = select_targets(Path::new(r"C:\c.mov"), &sel);
        assert_eq!(t, vec![PathBuf::from(r"C:\c.mov")]);
        let t = select_targets(Path::new(r"C:\c.mov"), &[]);
        assert_eq!(t, vec![PathBuf::from(r"C:\c.mov")]);
    }

    #[test]
    fn tracker_swallows_down_and_matching_up() {
        let mut t = ButtonTracker::new();
        let d = t.decide(MouseMsg::MButtonDown, 0, 0, false, || {
            HitKind::ExplorerFileList
        });
        assert_eq!(d, HookDecision::SwallowDown(HitKind::ExplorerFileList));
        assert_eq!(
            t.decide(MouseMsg::MButtonUp, 0, 0, false, || unreachable!()),
            HookDecision::SwallowUp
        );
        // 다음 떼기는 짝이 없으므로 통과
        assert_eq!(
            t.decide(MouseMsg::MButtonUp, 0, 0, false, || unreachable!()),
            HookDecision::Pass
        );
    }

    #[test]
    fn tracker_passes_other_windows_and_their_up() {
        let mut t = ButtonTracker::new();
        assert_eq!(
            t.decide(MouseMsg::MButtonDown, 0, 0, false, || HitKind::Other),
            HookDecision::Pass
        );
        assert_eq!(
            t.decide(MouseMsg::MButtonUp, 0, 0, false, || unreachable!()),
            HookDecision::Pass
        );
    }

    #[test]
    fn tracker_paused_does_not_classify() {
        let mut t = ButtonTracker::new();
        assert_eq!(
            t.decide(MouseMsg::MButtonDown, 0, 0, true, || unreachable!()),
            HookDecision::Pass
        );
        assert_eq!(
            t.decide(MouseMsg::MButtonUp, 0, 0, true, || unreachable!()),
            HookDecision::Pass
        );
    }

    #[test]
    fn tracker_passes_own_injected_input() {
        let mut t = ButtonTracker::new();
        let d = t.decide(
            MouseMsg::MButtonDown,
            LLMHF_INJECTED,
            OWN_INPUT_MARK,
            false,
            || unreachable!(),
        );
        assert_eq!(d, HookDecision::Pass);
        // 다른 프로그램이 주입한 입력은 일반 입력처럼 판정한다
        let d = t.decide(MouseMsg::MButtonDown, LLMHF_INJECTED, 7, false, || {
            HitKind::Desktop
        });
        assert_eq!(d, HookDecision::SwallowDown(HitKind::Desktop));
        // 우리가 다시 보낸 떼기는 상태를 바꾸지 않고 통과
        let d = t.decide(
            MouseMsg::MButtonUp,
            LLMHF_INJECTED,
            OWN_INPUT_MARK,
            false,
            || unreachable!(),
        );
        assert_eq!(d, HookDecision::Pass);
        assert_eq!(
            t.decide(MouseMsg::MButtonUp, 0, 0, false, || unreachable!()),
            HookDecision::SwallowUp
        );
    }

    #[test]
    fn tracker_second_down_resets_state() {
        let mut t = ButtonTracker::new();
        t.decide(MouseMsg::MButtonDown, 0, 0, false, || HitKind::Desktop);
        // 떼기 없이 다시 누름(다른 창)
        assert_eq!(
            t.decide(MouseMsg::MButtonDown, 0, 0, false, || HitKind::Other),
            HookDecision::Pass
        );
        assert_eq!(
            t.decide(MouseMsg::MButtonUp, 0, 0, false, || unreachable!()),
            HookDecision::Pass
        );
        assert_eq!(
            t.decide(MouseMsg::Other, 0, 0, false, || unreachable!()),
            HookDecision::Pass
        );
    }
}
