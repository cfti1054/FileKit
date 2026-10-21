//! 앱 시작과 모드별 흐름.
//!
//! - **Tray**: 파이프 서버 첫 인스턴스(단일 인스턴스 판정) → 트레이 → radial 창 미리 만들기 → 트리거 스레드.
//! - **Window**: 트레이가 있으면 `show_main_window`를 보내고 끝. 없으면 단독 실행하고 main을 보인다.
//! - **ConvertAsk**: 트레이가 있으면 `show_menu`를 보내고 끝. 없으면 창 모드로 시작하고
//!   `get_launch_files`에 파일을 넣는다.
//!
//! 모든 모드: 엔진 탐색(`locate`)·확인(`probe`)을 백그라운드 스레드에서 돌린 뒤 `JobQueue`를 만든다.
//! 작업 이벤트 sink 순서: `app.emit(job:event)` → 파이프 구독자 broadcast → 트레이 바쁨 → 알림.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use convtool_core::pipe::{ClientKind, ClientMessage, ServerMessage};
use convtool_core::{EngineConfig, EngineSet, EventSink, JobEvent, JobQueue, QueueConfig};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tokio::net::windows::named_pipe::NamedPipeServer;

use crate::args::AppMode;
use crate::events::{CloseReason, EV_JOB, WINDOW_MAIN};
use crate::pipe::client::{self, SendError};
use crate::pipe::server;
use crate::state::{lock, AppState, Core, RunKind};

pub fn run(mode: AppMode) -> ExitCode {
    // 창을 만들기 전에 AUMID를 정한다(토스트 발신자 이름, 작업 표시줄 묶음).
    crate::notify::set_process_aumid();
    match mode {
        AppMode::Tray => run_tray(),
        AppMode::Window => {
            let msg = ClientMessage::ShowMainWindow {
                request_id: 1,
                files: vec![],
            };
            match forward(ClientKind::Window, msg) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => run_app(RunKind::Window, vec![], None),
            }
        }
        AppMode::ConvertAsk { files } => {
            let msg = ClientMessage::ShowMenu {
                request_id: 1,
                primary: files.first().cloned(),
                files: files.clone(),
            };
            match forward(ClientKind::Shell, msg) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => run_app(RunKind::Window, files, None),
            }
        }
    }
}

/// 트레이에 요청을 넘긴다. Err면 호출하는 쪽이 단독 실행으로 넘어간다.
fn forward(kind: ClientKind, msg: ClientMessage) -> Result<(), SendError> {
    let r = tauri::async_runtime::block_on(client::send_one(kind, msg));
    match &r {
        Ok(ServerMessage::Ack { ok: true, .. }) => {
            tracing::info!("실행 중인 트레이에 요청을 넘겼습니다");
            return Ok(());
        }
        Ok(other) => tracing::warn!("트레이 응답이 예상과 다릅니다: {other:?}"),
        Err(SendError::NotRunning) => tracing::info!("트레이가 없어 단독으로 실행합니다"),
        Err(e) => tracing::warn!("트레이에 요청을 넘기지 못해 단독으로 실행합니다: {e}"),
    }
    match r {
        Ok(other) => Err(SendError::Protocol(format!("{other:?}"))),
        Err(e) => Err(e),
    }
}

fn run_tray() -> ExitCode {
    // NamedPipeServer는 tokio 리액터에 등록되므로 tauri 런타임 안에서 만든다.
    let first = tauri::async_runtime::block_on(async { server::bind_first() });
    match first {
        Ok(s) => run_app(RunKind::Tray, vec![], Some(s)),
        Err(e) if server::is_already_running(&e) => {
            tracing::info!("트레이가 이미 실행 중입니다. 종료합니다");
            ExitCode::SUCCESS
        }
        Err(e) => {
            tracing::error!("파이프 서버를 열지 못했습니다: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_app(kind: RunKind, launch_files: Vec<PathBuf>, pipe: Option<NamedPipeServer>) -> ExitCode {
    let state = AppState::new(kind, launch_files);
    let mut pipe = pipe;

    let app = tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            crate::commands::get_formats,
            crate::commands::submit_convert,
            crate::commands::cancel_job,
            crate::commands::get_engines,
            crate::commands::get_presets,
            crate::commands::get_launch_files,
            crate::commands::radial_ready,
            crate::commands::radial_select,
            crate::commands::radial_cancel,
        ])
        .on_window_event(move |window, event| {
            // 트레이 모드에서 메인 창 닫기 = 숨기기 (트레이는 계속 산다)
            if let WindowEvent::CloseRequested { api, .. } = event {
                if kind == RunKind::Tray && window.label() == WINDOW_MAIN {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            start_core(&handle);
            start_theme_watch(&handle);
            match kind {
                RunKind::Window => crate::tray::show_main(&handle),
                RunKind::Tray => {
                    crate::tray::create(&handle)?;
                    crate::overlay::create_radial(&handle)?;
                    if let Some(p) = pipe.take() {
                        tauri::async_runtime::spawn(server::serve(p, handle.clone()));
                    }
                    match start_trigger(&handle) {
                        Ok(t) => *lock(&handle.state::<AppState>().trigger) = Some(t),
                        Err(e) => tracing::error!("트리거 스레드를 시작하지 못했습니다: {e}"),
                    }
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!());

    let app = match app {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("앱을 시작하지 못했습니다: {e}");
            return ExitCode::from(1);
        }
    };

    app.run(move |handle, event| match event {
        RunEvent::ExitRequested { api, code, .. } => {
            // 트레이 모드: 창을 모두 닫아도(code 없음) 계속 산다. 트레이 "종료"(app.exit)는 code가 있다.
            if kind == RunKind::Tray && code.is_none() {
                api.prevent_exit();
            }
        }
        RunEvent::Exit => shutdown(handle),
        _ => {}
    });
    ExitCode::SUCCESS
}

/// 엔진 탐색·확인은 프로세스를 띄우므로(ffmpeg -encoders 등) 백그라운드에서 한다.
fn start_core(app: &AppHandle) {
    let handle = app.clone();
    let spawned = std::thread::Builder::new()
        .name("convtool-engines".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let mut engines = EngineSet::locate(&EngineConfig::from_env());
            engines.probe();
            let engines = Arc::new(engines);
            let queue = JobQueue::new(engines.clone(), QueueConfig::default(), make_sink(&handle));
            tracing::info!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                "엔진 확인 완료"
            );
            handle
                .state::<AppState>()
                .set_core(Arc::new(Core { engines, queue }));
        });
    if let Err(e) = spawned {
        tracing::error!("엔진 확인 스레드를 시작하지 못했습니다: {e}");
    }
}

/// 시스템 테마(강조색·고대비·애니메이션) 변경을 바로 `theme:changed`로 보낸다.
fn start_theme_watch(app: &AppHandle) {
    // 기준값을 먼저 읽어 둔다(첫 변경부터 알린다).
    crate::overlay::refresh_theme(app);
    let handle = app.clone();
    match crate::theme::watch(move || {
        crate::overlay::refresh_theme(&handle);
    }) {
        Ok(w) => *lock(&app.state::<AppState>().theme_watch) = Some(w),
        Err(e) => tracing::warn!("테마 감시를 시작하지 못했습니다(메뉴를 열 때만 반영): {e}"),
    }
}

fn make_sink(app: &AppHandle) -> EventSink {
    let handle = app.clone();
    Arc::new(move |ev: &JobEvent| {
        if let Err(e) = handle.emit(EV_JOB, ev) {
            tracing::warn!("job:event 보내기 실패: {e}");
        }
        let state = handle.state::<AppState>();
        // 구독자가 없으면 Err — 정상
        let _ = state.events.send(ev.clone());
        let changed = lock(&state.busy).apply(ev);
        if let Some(busy) = changed {
            crate::tray::set_busy(&handle, busy);
        }
        state.notifier.event(ev);
    })
}

/// 트리거 스레드 묶음 (훅 → 조정자 → 식별)
pub struct TriggerHandles {
    hook: crate::trigger::hook::HookThread,
    coordinator: Option<std::thread::JoinHandle<()>>,
}

impl TriggerHandles {
    fn stop(mut self) {
        // 훅 스레드가 끝나면 HookContext의 Sender가 닫혀 조정자 루프도 끝난다.
        self.hook.stop();
        if let Some(c) = self.coordinator.take() {
            let _ = c.join();
        }
    }
}

fn start_trigger(app: &AppHandle) -> std::io::Result<TriggerHandles> {
    use crate::events::OpenedBy;
    use crate::trigger::explorer::{self, IdentifyOutcome, IdentifyRequest};
    use crate::trigger::hook::{self, HookEvent};

    let state = app.state::<AppState>();
    let (htx, hrx) = std::sync::mpsc::channel::<HookEvent>();
    let gate = state.press_gate.clone();
    let hook = hook::start(htx, state.paused.clone(), gate.clone())?;

    let on_result_app = app.clone();
    let result_gate = gate.clone();
    let explorer = explorer::start(move |req: IdentifyRequest, outcome: IdentifyOutcome| {
        let identified_ms = req.at.elapsed().as_secs_f64() * 1000.0;
        match outcome {
            IdentifyOutcome::Files(found) => {
                let r = crate::overlay::open_menu(
                    &on_result_app,
                    found.files,
                    Some(found.primary),
                    Some((req.x, req.y)),
                    OpenedBy::Wheel,
                    Some(req.at),
                );
                match r {
                    Ok(_) => result_gate.menu_opened(),
                    Err(e) => {
                        // 지원하지 않는 파일, 엔진 확인 중 등. 메뉴를 열지 않고 원래 휠 클릭을 살린다.
                        tracing::info!(code = ?e.code, identified_ms, "원형 메뉴를 열지 않음: {}", e.message);
                        hook::replay(&result_gate);
                    }
                }
            }
            IdentifyOutcome::NotAnItem => {
                tracing::debug!(identified_ms, "파일 항목이 아님, 휠 클릭 다시 보내기");
                hook::replay(&result_gate);
            }
            IdentifyOutcome::Failed(reason) => {
                // UIPI, COM 오류, 이름 판정 불가. 오류로 띄우지 않는다.
                tracing::info!(
                    identified_ms,
                    "항목 식별 실패, 원래 동작으로 통과: {reason}"
                );
                hook::replay(&result_gate);
            }
        }
    })?;

    let app2 = app.clone();
    let coordinator = std::thread::Builder::new()
        .name("convtool-trigger".into())
        .spawn(move || {
            // 마지막으로 삼킨 누름 시각. 떼기를 그 누름으로 연 메뉴에만 넘긴다.
            let mut last_down: Option<std::time::Instant> = None;
            for ev in hrx {
                match ev {
                    HookEvent::MButtonDown {
                        x,
                        y,
                        hit,
                        root_hwnd,
                        at,
                    } => {
                        last_down = Some(at);
                        let sent = explorer.send(IdentifyRequest {
                            x,
                            y,
                            hit,
                            root_hwnd,
                            at,
                        });
                        if !sent {
                            tracing::warn!("식별 스레드가 끝나 휠 클릭을 다시 보냅니다");
                            hook::replay(&gate);
                        }
                    }
                    HookEvent::MButtonUp { x, y, at } => {
                        if let Some(down) = last_down {
                            crate::overlay::on_mbutton_up(&app2, (x, y), down, at);
                        }
                    }
                }
            }
            drop(explorer);
        })?;

    Ok(TriggerHandles {
        hook,
        coordinator: Some(coordinator),
    })
}

fn shutdown(app: &AppHandle) {
    tracing::info!("종료 정리");
    let state = app.state::<AppState>();
    crate::overlay::close(app, CloseReason::Shutdown);
    if let Some(t) = lock(&state.trigger).take() {
        t.stop();
    }
    if let Some(w) = lock(&state.theme_watch).take() {
        w.stop();
    }
    state.notifier.shutdown();
    if let Some(core) = state.take_core() {
        // 실행 중인 엔진 프로세스를 kill하고 작업 스레드를 join한다
        core.queue.shutdown();
    }
}
