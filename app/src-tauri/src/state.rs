//! 앱 공유 상태 (`tauri::State<AppState>`).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};

use convtool_core::{EngineSet, JobEvent, JobQueue};
use tokio::sync::{broadcast, watch};

use crate::busy::BusyTracker;
use crate::notify::{LogNotifier, Notifier};
use crate::pipe::MenuBatcher;
use crate::session::Sessions;

/// 앱이 어떤 방식으로 떠 있는지
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// 트레이 상주 (파이프 서버, 오버레이, 훅)
    Tray,
    /// 창 하나 (트레이가 없을 때의 단독 실행)
    Window,
}

/// 엔진 탐색·probe가 끝난 뒤 만드는 코어 묶음
pub struct Core {
    pub engines: Arc<EngineSet>,
    pub queue: JobQueue,
}

pub struct AppState {
    pub kind: RunKind,
    core_tx: watch::Sender<Option<Arc<Core>>>,
    core_rx: watch::Receiver<Option<Arc<Core>>>,
    /// 창 모드로 시작할 때 받은 파일 (`get_launch_files`가 한 번 읽고 비운다)
    pub launch_files: Mutex<Vec<PathBuf>>,
    pub sessions: Mutex<Sessions>,
    pub menu_batcher: Mutex<MenuBatcher>,
    /// 트레이 "원형 메뉴 일시 중지" (훅 판정이 읽는다)
    pub paused: Arc<AtomicBool>,
    pub busy: Mutex<BusyTracker>,
    /// 파이프 `subscribe` 연결로 보낼 작업 이벤트
    pub events: broadcast::Sender<JobEvent>,
    pub notifier: Arc<dyn Notifier>,
    /// 트리거 스레드 (트레이 모드). 종료할 때 멈춘다.
    pub trigger: Mutex<Option<crate::app::TriggerHandles>>,
}

/// 잠금이 오염(poison)돼도 안의 값을 쓴다. 상태는 단순 값이라 일관성이 깨지지 않는다.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    pub fn new(kind: RunKind, launch_files: Vec<PathBuf>) -> Self {
        let (core_tx, core_rx) = watch::channel(None);
        let (events, _) = broadcast::channel(256);
        Self {
            kind,
            core_tx,
            core_rx,
            launch_files: Mutex::new(launch_files),
            sessions: Mutex::new(Sessions::new()),
            menu_batcher: Mutex::new(MenuBatcher::new()),
            paused: Arc::new(AtomicBool::new(false)),
            busy: Mutex::new(BusyTracker::new()),
            events,
            notifier: Arc::new(LogNotifier),
            trigger: Mutex::new(None),
        }
    }

    /// 준비된 코어 (아직 엔진 확인 중이면 None)
    pub fn core_now(&self) -> Option<Arc<Core>> {
        self.core_rx.borrow().clone()
    }

    /// 코어가 준비될 때까지 기다린다
    pub async fn core(&self) -> Arc<Core> {
        let mut rx = self.core_rx.clone();
        loop {
            if let Some(c) = rx.borrow_and_update().clone() {
                return c;
            }
            if rx.changed().await.is_err() {
                // 보내는 쪽(self.core_tx)이 살아 있는 동안에는 오지 않는다. 혹시 오면 잠시 뒤 다시 본다.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }

    pub fn set_core(&self, core: Arc<Core>) {
        self.core_tx.send_replace(Some(core));
    }

    /// 종료: 코어를 내려 JobQueue Drop(shutdown: 모두 취소, 작업 스레드 join)이 일어나게 한다.
    pub fn take_core(&self) -> Option<Arc<Core>> {
        self.core_tx.send_replace(None)
    }
}
