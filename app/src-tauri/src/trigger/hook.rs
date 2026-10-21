//! 저수준 마우스 훅 스레드와 삼킨 클릭 다시 보내기.
//!
//! 훅 콜백은 설치한 스레드의 메시지 루프에서 불린다. 그래서 전용 스레드가 `GetMessageW` 루프를 돈다.
//! 콜백 안에서는 창 클래스 확인(`classify_point`)과 채널 전송만 한다. UIA·COM은 `explorer` 스레드가 한다.
//! 로그·잠금도 하지 않는다(`LowLevelHooksTimeout`을 넘기면 Windows가 훅을 조용히 끊는다).
//!
//! 휠 버튼 누름을 삼킨 뒤의 흐름:
//! - 파일이면 원형 메뉴(`PressGate::menu_opened`), 짝 떼기는 삼켜 제스처 판정으로 넘긴다.
//! - 파일이 아니거나 식별에 실패하면 `replay`로 누름(필요하면 떼기도)을 `SendInput`으로 다시 보낸다.
//!   `dwExtraInfo = OWN_INPUT_MARK`라 훅이 다시 삼키지 않는다.

use std::time::Instant;

use super::classify::HitKind;

/// 훅 콜백 → 트리거 조정자로 넘기는 이벤트. 콜백 안에서 만들므로 할당이 없는 작은 값만 담는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// 후보 창 위에서 휠 버튼을 눌렀고 삼켰다. 좌표는 화면 물리 px.
    MButtonDown {
        x: i32,
        y: i32,
        hit: HitKind,
        /// 최상위 창 HWND (원시 값)
        root_hwnd: isize,
        at: Instant,
    },
    /// 삼킨 누름의 짝이 되는 떼기 (제스처 방식 판정용)
    MButtonUp { x: i32, y: i32, at: Instant },
}

#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Instant;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEINPUT, MOUSE_EVENT_FLAGS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetAncestor, GetClassNameW, GetMessageW, PeekMessageW, PostThreadMessageW,
        SetWindowsHookExW, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HC_ACTION, MSG,
        MSLLHOOKSTRUCT, PM_NOREMOVE, WH_MOUSE_LL, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_QUIT,
    };

    use super::HookEvent;
    use crate::trigger::classify::{
        classify_utf16, ButtonTracker, HitKind, HookDecision, MouseMsg, PressGate,
        CLASS_NAME_BUF_LEN, OWN_INPUT_MARK,
    };

    /// 훅 콜백이 쓰는 상태. 훅 스레드의 thread_local에만 산다.
    pub struct HookContext {
        tracker: ButtonTracker,
        tx: Sender<HookEvent>,
        paused: Arc<AtomicBool>,
    }

    impl HookContext {
        pub fn new(tx: Sender<HookEvent>, paused: Arc<AtomicBool>, gate: Arc<PressGate>) -> Self {
            Self {
                tracker: ButtonTracker::with_gate(gate),
                tx,
                paused,
            }
        }

        /// 훅 콜백 본문. `MSLLHOOKSTRUCT`에서 꺼낸 값만 받는다.
        /// 반환값이 `Pass`면 `CallNextHookEx`, 그 밖이면 `LRESULT(1)`로 삼킨다.
        pub fn on_mouse(
            &mut self,
            msg: MouseMsg,
            x: i32,
            y: i32,
            flags: u32,
            extra_info: usize,
        ) -> HookDecision {
            // 트레이 "원형 메뉴 일시 중지"면 창 클래스도 읽지 않고 바로 통과한다.
            let paused = self.paused.load(Ordering::Relaxed);
            let mut root = 0isize;
            let decision = self.tracker.decide(msg, flags, extra_info, paused, || {
                let (hit, r) = classify_point(x, y);
                root = r;
                hit
            });
            let at = Instant::now();
            match decision {
                HookDecision::SwallowDown(hit) => {
                    // 받는 쪽이 끝났으면(앱 종료 중) 보낼 곳이 없다. 클릭은 이미 삼켰으므로 무시한다.
                    let _ = self.tx.send(HookEvent::MButtonDown {
                        x,
                        y,
                        hit,
                        root_hwnd: root,
                        at,
                    });
                }
                HookDecision::SwallowUp => {
                    let _ = self.tx.send(HookEvent::MButtonUp { x, y, at });
                }
                HookDecision::Pass => {}
            }
            decision
        }
    }

    thread_local! {
        static CTX: RefCell<Option<HookContext>> = const { RefCell::new(None) };
    }

    /// `WH_MOUSE_LL` 콜백. 휠 버튼 누름·떼기만 본다.
    unsafe extern "system" fn low_level_mouse_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code == HC_ACTION as i32 {
            let msg = match wparam.0 as u32 {
                WM_MBUTTONDOWN => MouseMsg::MButtonDown,
                WM_MBUTTONUP => MouseMsg::MButtonUp,
                _ => MouseMsg::Other,
            };
            if msg != MouseMsg::Other && lparam.0 != 0 {
                // SAFETY: HC_ACTION일 때 lparam은 MSLLHOOKSTRUCT를 가리킨다(문서 계약).
                let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
                let decision = CTX.with(|c| {
                    // 재진입(콜백 안에서 다시 콜백)은 없지만, 있더라도 패닉 대신 통과시킨다.
                    c.try_borrow_mut().ok().and_then(|mut ctx| {
                        ctx.as_mut().map(|ctx| {
                            ctx.on_mouse(msg, info.pt.x, info.pt.y, info.flags, info.dwExtraInfo)
                        })
                    })
                });
                if matches!(
                    decision,
                    Some(HookDecision::SwallowDown(_) | HookDecision::SwallowUp)
                ) {
                    return LRESULT(1);
                }
            }
        }
        // SAFETY: 받은 인자를 그대로 넘긴다. 첫 인자(hhk)는 무시된다.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// 창 클래스 이름을 호출자의 스택 버퍼에 읽어 NUL을 뺀 슬라이스로 돌려준다. 힙 할당 없음.
    fn class_name(hwnd: HWND, buf: &mut [u16; CLASS_NAME_BUF_LEN]) -> &[u16] {
        // SAFETY: buf는 유효한 쓰기 버퍼다. 반환값은 NUL을 뺀 글자 수이고, hwnd가 무효면 0이다.
        let n = unsafe { GetClassNameW(hwnd, buf) };
        let n = usize::try_from(n).unwrap_or(0).min(buf.len());
        &buf[..n]
    }

    /// 커서 아래 창을 분류한다. 훅 콜백 안에서 부르는 유일한 Win32 조회다(UIA·COM 없음).
    /// 콜백 경로이므로 힙 할당을 하지 않는다(고정 버퍼 + UTF-16 상수 비교).
    pub fn classify_point(x: i32, y: i32) -> (HitKind, isize) {
        // SAFETY: 단순 조회 API. 실패하면 null HWND를 돌려준다.
        let hit = unsafe { WindowFromPoint(POINT { x, y }) };
        if hit.is_invalid() {
            return (HitKind::Other, 0);
        }
        // SAFETY: 위와 같다.
        let root = unsafe { GetAncestor(hit, GA_ROOT) };
        let root = if root.is_invalid() { hit } else { root };
        let mut root_buf = [0u16; CLASS_NAME_BUF_LEN];
        let mut hit_buf = [0u16; CLASS_NAME_BUF_LEN];
        let kind = classify_utf16(
            class_name(root, &mut root_buf),
            class_name(hit, &mut hit_buf),
        );
        (kind, root.0 as isize)
    }

    fn middle_input(flags: MOUSE_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: OWN_INPUT_MARK,
                },
            },
        }
    }

    fn send_middle(flags: MOUSE_EVENT_FLAGS) -> bool {
        let input = [middle_input(flags)];
        // SAFETY: input은 유효한 INPUT 배열이고 cbsize는 INPUT 크기다.
        let n = unsafe { SendInput(&input, std::mem::size_of::<INPUT>() as i32) };
        n == 1
    }

    /// 삼킨 휠 클릭을 원래 동작으로 되살린다. 커서는 옮기지 않는다(지금 커서 위치에 주입).
    ///
    /// 누름을 먼저 주입한 뒤 `PressGate`로 버튼이 아직 눌려 있는지 판정한다(순서 보장은 `PressGate` 문서).
    /// UIPI로 관리자 권한 창에 주입이 막혀도 오류로 띄우지 않고 로그만 남긴다.
    pub fn replay(gate: &PressGate) {
        if !send_middle(MOUSEEVENTF_MIDDLEDOWN) {
            tracing::info!("휠 클릭 다시 보내기 실패 (SendInput 누름, UIPI 가능성)");
        }
        if gate.replayed_down() && !send_middle(MOUSEEVENTF_MIDDLEUP) {
            tracing::info!("휠 클릭 다시 보내기 실패 (SendInput 떼기)");
        }
    }

    /// 훅 스레드 핸들. `stop`이나 Drop에서 `WM_QUIT`을 보내고 join한다.
    pub struct HookThread {
        thread_id: u32,
        join: Option<JoinHandle<()>>,
    }

    /// 훅 스레드를 시작하고 `WH_MOUSE_LL`을 설치한다. 스레드가 끝날 때(앱 종료) 반드시 해제한다.
    pub fn start(
        tx: Sender<HookEvent>,
        paused: Arc<AtomicBool>,
        gate: Arc<PressGate>,
    ) -> io::Result<HookThread> {
        spawn(tx, paused, gate, true)
    }

    /// `install = false`는 테스트용(사용자 입력을 가로채지 않고 메시지 루프만 돈다).
    fn spawn(
        tx: Sender<HookEvent>,
        paused: Arc<AtomicBool>,
        gate: Arc<PressGate>,
        install: bool,
    ) -> io::Result<HookThread> {
        let (id_tx, id_rx) = mpsc::channel::<io::Result<u32>>();
        let join = std::thread::Builder::new()
            .name("convtool-hook".into())
            .spawn(move || {
                let mut msg = MSG::default();
                // SAFETY: 이 스레드의 메시지 큐를 만들어 PostThreadMessageW가 성공하게 한다.
                unsafe {
                    let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
                }
                CTX.with(|c| *c.borrow_mut() = Some(HookContext::new(tx, paused, gate)));

                let hook = if install {
                    // SAFETY: 콜백은 이 모듈의 extern "system" 함수이고, 모듈 핸들은 이 exe다.
                    let r = unsafe {
                        GetModuleHandleW(None).and_then(|m| {
                            SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), m, 0)
                        })
                    };
                    match r {
                        Ok(h) => Some(h),
                        Err(e) => {
                            let _ = id_tx
                                .send(Err(io::Error::other(format!("WH_MOUSE_LL 설치 실패: {e}"))));
                            return;
                        }
                    }
                } else {
                    None
                };
                // SAFETY: 인자 없는 조회.
                let _ = id_tx.send(Ok(unsafe { GetCurrentThreadId() }));
                tracing::info!(installed = hook.is_some(), "훅 스레드 시작");

                loop {
                    // SAFETY: msg는 유효한 출력 버퍼. WM_QUIT이면 0, 오류면 -1.
                    // 훅 콜백은 이 GetMessageW 안에서 불린다. 이 스레드에는 창이 없어 Dispatch할 것이 없다.
                    let r = unsafe { GetMessageW(&mut msg, HWND::default(), 0, 0) };
                    if r.0 == 0 || r.0 == -1 {
                        break;
                    }
                }
                if let Some(h) = hook {
                    // SAFETY: 위에서 설치한 훅. 앱 종료 시 해제(spec 6장).
                    if let Err(e) = unsafe { UnhookWindowsHookEx(h) } {
                        tracing::warn!("WH_MOUSE_LL 해제 실패: {e}");
                    }
                }
                CTX.with(|c| c.borrow_mut().take());
                tracing::info!("훅 스레드 종료");
            })?;
        let thread_id = id_rx
            .recv()
            .map_err(|_| io::Error::other("훅 스레드가 ID를 보내기 전에 끝났습니다"))??;
        Ok(HookThread {
            thread_id,
            join: Some(join),
        })
    }

    impl HookThread {
        pub fn stop(mut self) {
            self.stop_inner();
        }

        fn stop_inner(&mut self) {
            if let Some(join) = self.join.take() {
                // SAFETY: 메시지 큐가 있는 스레드에 WM_QUIT을 보낸다.
                let r =
                    unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
                if let Err(e) = r {
                    tracing::warn!("훅 스레드에 WM_QUIT 보내기 실패: {e}");
                    return;
                }
                let _ = join.join();
            }
        }
    }

    impl Drop for HookThread {
        fn drop(&mut self) {
            self.stop_inner();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn thread_starts_and_stops_without_hook() {
            let (tx, _rx) = mpsc::channel();
            let t = spawn(
                tx,
                Arc::new(AtomicBool::new(false)),
                Arc::new(PressGate::new()),
                false,
            )
            .unwrap();
            t.stop();
        }

        #[test]
        fn context_paused_passes_without_events() {
            let (tx, rx) = mpsc::channel();
            let mut ctx = HookContext::new(
                tx,
                Arc::new(AtomicBool::new(true)),
                Arc::new(PressGate::new()),
            );
            // 일시 중지 중이면 classify_point를 부르지 않고 통과
            assert_eq!(
                ctx.on_mouse(MouseMsg::MButtonDown, 0, 0, 0, 0),
                HookDecision::Pass
            );
            assert_eq!(
                ctx.on_mouse(MouseMsg::MButtonUp, 0, 0, 0, 0),
                HookDecision::Pass
            );
            assert!(rx.try_recv().is_err());
        }

        #[test]
        fn replay_input_is_marked() {
            let i = middle_input(MOUSEEVENTF_MIDDLEDOWN);
            assert_eq!(i.r#type, INPUT_MOUSE);
            // SAFETY: 위에서 mi로 만든 값
            let mi = unsafe { i.Anonymous.mi };
            assert_eq!(mi.dwExtraInfo, OWN_INPUT_MARK);
            assert_eq!(mi.dwFlags, MOUSEEVENTF_MIDDLEDOWN);
        }
    }
}
