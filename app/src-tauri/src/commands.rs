//! Tauri 명령 9개 (설계 계획 경계 (3)). `app/ui/src/shared/ipc.ts`의 invoke 이름·인자와 1:1이다.
//!
//! 모두 `rename_all = "snake_case"`: JS 인자 객체도 snake_case (`{ job_id }`, `{ session_id, format_id }`).
//! 실패하면 `ErrorInfo`로 reject한다.

use std::path::PathBuf;

use convtool_core::{
    presets, query_formats, BatchTicket, CoreError, EngineInfo, ErrorInfo, FormatsReport, JobId,
    JobRequest, PresetId, PresetInfo,
};
use tauri::{AppHandle, State};

use crate::events::RadialOpenPayload;
use crate::overlay::{self, internal};
use crate::session::SelectOutcome;
use crate::state::{lock, AppState};

#[tauri::command(rename_all = "snake_case")]
pub async fn get_formats(
    state: State<'_, AppState>,
    files: Vec<PathBuf>,
    primary: Option<PathBuf>,
) -> Result<FormatsReport, ErrorInfo> {
    let core = state.core().await;
    query_formats(&files, primary.as_deref(), &core.engines).map_err(|e| e.info())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn submit_convert(
    state: State<'_, AppState>,
    request: JobRequest,
) -> Result<BatchTicket, ErrorInfo> {
    let core = state.core().await;
    core.queue.submit(request).map_err(|e| e.info())
}

#[tauri::command(rename_all = "snake_case")]
pub fn cancel_job(state: State<'_, AppState>, job_id: JobId) -> bool {
    state.core_now().is_some_and(|c| c.queue.cancel(job_id))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_engines(state: State<'_, AppState>) -> Result<Vec<EngineInfo>, ErrorInfo> {
    Ok(state.core().await.engines.status())
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_presets() -> Vec<PresetInfo> {
    presets::list()
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_launch_files(state: State<'_, AppState>) -> Vec<PathBuf> {
    std::mem::take(&mut *lock(&state.launch_files))
}

#[tauri::command(rename_all = "snake_case")]
pub fn radial_ready(state: State<'_, AppState>) -> Option<RadialOpenPayload> {
    lock(&state.sessions).ready_payload()
}

#[tauri::command(rename_all = "snake_case")]
pub fn radial_select(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: u64,
    format_id: String,
) -> Result<(), ErrorInfo> {
    let outcome = lock(&state.sessions).select(session_id, &format_id);
    if outcome == SelectOutcome::Stale {
        tracing::debug!(session_id, "오래된 세션의 radial_select 무시");
        return Ok(());
    }
    overlay::hide(&app);
    let result = match outcome {
        SelectOutcome::Submit { files, to } => {
            let core = state
                .core_now()
                .ok_or_else(|| internal("엔진을 확인하는 중입니다"))?;
            let request = JobRequest {
                files,
                to,
                // TODO(F-08): 설정 화면의 기본 프리셋
                preset: PresetId::default(),
                out_dir: None,
                overwrite: false,
            };
            core.queue.submit(request).map(|_| ()).map_err(|e| e.info())
        }
        SelectOutcome::UnknownFormat => {
            Err(CoreError::InvalidInput(format!("메뉴에 없는 형식입니다: {format_id}")).info())
        }
        SelectOutcome::Unavailable(message) => Err(CoreError::InvalidInput(message).info()),
        SelectOutcome::Stale => Ok(()),
    };
    if let Err(e) = &result {
        state.notifier.error(e);
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
pub fn radial_cancel(app: AppHandle, state: State<'_, AppState>, session_id: u64) {
    let hide = {
        let mut s = lock(&state.sessions);
        // 오래된 세션의 취소가 새로 열린 메뉴를 숨기지 않게 한다
        s.cancel(session_id) || s.current_id().is_none()
    };
    if hide {
        overlay::hide(&app);
    }
}
