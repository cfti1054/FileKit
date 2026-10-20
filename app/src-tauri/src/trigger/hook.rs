//! 저수준 마우스 훅 스레드.
//!
//! 훅 콜백은 설치한 스레드의 메시지 루프에서 불린다. 그래서 전용 스레드가 `GetMessageW` 루프를 돈다.
//! 콜백 안에서는 창 클래스 확인(`classify_point`)과 채널 전송만 한다. UIA·COM은 `explorer` 스레드가 한다.
//!
//! **지금은 훅을 설치하지 않는다(TODO(F-01)).** 항목 식별(`explorer::identify`)이 TODO인 상태에서
//! 휠 클릭을 삼키면 탐색기·바탕화면에서 사용자의 휠 클릭이 그냥 사라지기 때문이다.
//! 스레드와 메시지 루프, 판정(`classify::ButtonTracker`)과 이벤트 채널까지만 만들어 둔다.

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
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::Instant;

    use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetClassNameW, GetMessageW, PeekMessageW, PostThreadMessageW, WindowFromPoint,
        GA_ROOT, MSG, PM_NOREMOVE, WM_QUIT,
    };

    use super::HookEvent;
    use crate::trigger::classify::{
        classify_utf16, ButtonTracker, HitKind, HookDecision, MouseMsg, CLASS_NAME_BUF_LEN,
    };

    /// 훅 콜백이 쓰는 상태. 훅 스레드 안에서만 산다(thread_local로 둘 예정).
    pub struct HookContext {
        tracker: ButtonTracker,
        tx: Sender<HookEvent>,
        paused: Arc<AtomicBool>,
    }

    impl HookContext {
        pub fn new(tx: Sender<HookEvent>, paused: Arc<AtomicBool>) -> Self {
            Self {
                tracker: ButtonTracker::new(),
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

    /// 훅 스레드 핸들. `stop`이나 Drop에서 `WM_QUIT`을 보내고 join한다.
    pub struct HookThread {
        thread_id: u32,
        join: Option<JoinHandle<()>>,
    }

    /// 훅 스레드를 시작한다. 지금은 메시지 루프만 돌고 훅은 설치하지 않는다(TODO(F-01)).
    pub fn start(tx: Sender<HookEvent>, paused: Arc<AtomicBool>) -> io::Result<HookThread> {
        let (id_tx, id_rx) = mpsc::channel::<u32>();
        let join = std::thread::Builder::new()
            .name("convtool-hook".into())
            .spawn(move || {
                let mut msg = MSG::default();
                // SAFETY: 이 스레드의 메시지 큐를 만들어 PostThreadMessageW가 성공하게 한다.
                unsafe {
                    let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
                }
                // SAFETY: 인자 없는 조회.
                let _ = id_tx.send(unsafe { GetCurrentThreadId() });

                let _ctx = HookContext::new(tx, paused);
                // TODO(F-01): 훅 설치. explorer::identify가 구현된 뒤에 연다.
                //   thread_local! { static CTX: RefCell<Option<HookContext>> }에 _ctx를 넣고
                //   let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), HINSTANCE(모듈), 0)?;
                //   low_level_mouse_proc(code, wparam, lparam):
                //     code < 0 → CallNextHookEx
                //     wparam = WM_MBUTTONDOWN/WM_MBUTTONUP → MSLLHOOKSTRUCT(pt, flags, dwExtraInfo)로
                //     CTX.on_mouse(...) → Pass면 CallNextHookEx, 아니면 LRESULT(1)
                //   콜백 안에서 로그·잠금·할당을 하지 않는다(LowLevelHooksTimeout 넘기면 Windows가 훅을 끊는다).
                //   루프가 끝나면 UnhookWindowsHookEx(hook). 앱 종료 시 반드시 해제(spec 6장 "앱 종료 시 해제").
                //   UIPI: 관리자 권한 창 위의 입력은 오지 않는다. 오류로 띄우지 않는다.
                tracing::info!(
                    "훅 스레드 시작 (WH_MOUSE_LL 설치는 TODO(F-01): 휠 클릭을 가로채지 않음)"
                );

                loop {
                    // SAFETY: msg는 유효한 출력 버퍼. WM_QUIT이면 0, 오류면 -1.
                    let r = unsafe { GetMessageW(&mut msg, HWND::default(), 0, 0) };
                    if r.0 == 0 || r.0 == -1 {
                        break;
                    }
                }
                tracing::info!("훅 스레드 종료");
            })?;
        let thread_id = id_rx
            .recv()
            .map_err(|_| io::Error::other("훅 스레드가 ID를 보내기 전에 끝났습니다"))?;
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
        fn thread_starts_and_stops() {
            let (tx, _rx) = mpsc::channel();
            let t = start(tx, Arc::new(AtomicBool::new(false))).unwrap();
            t.stop();
        }

        #[test]
        fn context_sends_events_for_candidates_only() {
            let (tx, rx) = mpsc::channel();
            let mut ctx = HookContext::new(tx, Arc::new(AtomicBool::new(true)));
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
    }
}
