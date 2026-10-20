//! 작업 요청·이벤트·결과 타입과 작업 큐.
//!
//! 큐는 std 스레드 + Mutex/Condvar로 만든다(tokio 쓰지 않음). 전체 동시 실행은
//! `QueueConfig::max_parallel`, LibreOffice를 쓰는 작업은 `office_parallel`로 제한한다.
//! 이벤트 sink는 큐 잠금을 쥐지 않은 채로 부른다(sink 안에서 큐를 불러도 교착하지 않는다).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::engines::{
    self, capture_stdout, ffmpeg, run_step_with, CancelToken, EngineId, EngineSet, PlanInput, Step,
};
use crate::error::{CoreError, ErrorCode, ErrorInfo};
use crate::formats;
use crate::naming;
use crate::presets::PresetId;

pub type JobId = u64;
pub type BatchId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRequest {
    pub files: Vec<PathBuf>,
    pub to: String,
    #[serde(default)]
    pub preset: PresetId,
    /// 없으면 원본 폴더. 있으면 create_dir_all
    #[serde(default)]
    pub out_dir: Option<PathBuf>,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedJob {
    pub job_id: JobId,
    pub input: PathBuf,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RejectedFile {
    pub input: PathBuf,
    pub error: ErrorInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchTicket {
    pub batch_id: BatchId,
    pub jobs: Vec<QueuedJob>,
    pub rejected: Vec<RejectedFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    Queued {
        batch_id: BatchId,
        job_id: JobId,
        input: PathBuf,
        to: String,
    },
    Rejected {
        batch_id: BatchId,
        input: PathBuf,
        error: ErrorInfo,
    },
    Started {
        batch_id: BatchId,
        job_id: JobId,
        engine: EngineId,
    },
    /// progress: null = 진행 중, 비율 모름
    Progress {
        batch_id: BatchId,
        job_id: JobId,
        progress: Option<f64>,
    },
    Done {
        batch_id: BatchId,
        job_id: JobId,
        input: PathBuf,
        output: PathBuf,
    },
    Failed {
        batch_id: BatchId,
        job_id: JobId,
        input: PathBuf,
        error: ErrorInfo,
    },
    Canceled {
        batch_id: BatchId,
        job_id: JobId,
        input: PathBuf,
    },
    BatchDone {
        batch_id: BatchId,
        succeeded: u32,
        failed: u32,
        canceled: u32,
        rejected: u32,
        exit_code: u8,
    },
}

pub type EventSink = Arc<dyn Fn(&JobEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct QueueConfig {
    /// 기본 2
    pub max_parallel: usize,
    /// 기본 1 (office를 쓰는 작업 = md→pdf 포함)
    pub office_parallel: usize,
    /// 기본 100ms (작업마다 progress 이벤트 최대 빈도)
    pub progress_interval: Duration,
}

impl Default for QueueConfig {
    fn default() -> Self {
        QueueConfig {
            max_parallel: 2,
            office_parallel: 1,
            progress_interval: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutcomeKind {
    Done { output: PathBuf },
    Failed { error: ErrorInfo },
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobOutcome {
    pub job_id: JobId,
    pub input: PathBuf,
    pub outcome: OutcomeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchSummary {
    pub batch_id: BatchId,
    pub outcomes: Vec<JobOutcome>,
    pub rejected: Vec<RejectedFile>,
}

impl BatchSummary {
    /// 0: 거절·실패·취소 없음
    /// 실행된 작업이 0개: 거절 중 engine_missing|codec_missing이 있으면 3, 아니면 2
    /// 그 밖(하나라도 실패·취소·거절): 1
    pub fn exit_code(&self) -> u8 {
        let all_done = self
            .outcomes
            .iter()
            .all(|o| matches!(o.outcome, OutcomeKind::Done { .. }));
        if self.rejected.is_empty() && all_done {
            return 0;
        }
        if self.outcomes.is_empty() {
            let engine_problem = self.rejected.iter().any(|r| {
                matches!(
                    r.error.code,
                    ErrorCode::EngineMissing | ErrorCode::CodecMissing
                )
            });
            return if engine_problem { 3 } else { 2 };
        }
        1
    }

    fn batch_done_event(&self) -> JobEvent {
        let count = |f: fn(&OutcomeKind) -> bool| {
            u32::try_from(self.outcomes.iter().filter(|o| f(&o.outcome)).count())
                .unwrap_or(u32::MAX)
        };
        JobEvent::BatchDone {
            batch_id: self.batch_id,
            succeeded: count(|o| matches!(o, OutcomeKind::Done { .. })),
            failed: count(|o| matches!(o, OutcomeKind::Failed { .. })),
            canceled: count(|o| matches!(o, OutcomeKind::Canceled)),
            rejected: u32::try_from(self.rejected.len()).unwrap_or(u32::MAX),
            exit_code: self.exit_code(),
        }
    }
}

// ===== 스케줄러 (순수 함수) =====

/// 대기열에서 다음에 실행할 작업의 인덱스를 고른다.
///
/// `pending_office[i]`는 i번째 대기 작업이 LibreOffice를 쓰는지다. 앞에서부터(FIFO) 고르되,
/// office 제한에 걸린 작업은 건너뛰고 뒤의 다른 작업을 먼저 실행한다.
/// 제한 값이 0이면 1로 본다(굶주림 방지).
pub(crate) fn pick_next(
    pending_office: &[bool],
    running: usize,
    running_office: usize,
    max_parallel: usize,
    office_parallel: usize,
) -> Option<usize> {
    if running >= max_parallel.max(1) {
        return None;
    }
    let office_free = running_office < office_parallel.max(1);
    pending_office
        .iter()
        .position(|&office| !office || office_free)
}

/// 진행률을 소수 셋째 자리에서 반올림하고 0..=1로 자른다.
pub(crate) fn round_progress(v: f64) -> f64 {
    (v.clamp(0.0, 1.0) * 1000.0).round() / 1000.0
}

/// 작업마다 progress 이벤트를 `interval`에 한 번 이하로 줄인다. 같은 값은 다시 보내지 않는다.
#[derive(Debug)]
pub(crate) struct ProgressThrottle {
    interval: Duration,
    last_at: Option<Instant>,
    last_value: Option<Option<f64>>,
}

impl ProgressThrottle {
    pub(crate) fn new(interval: Duration) -> Self {
        ProgressThrottle {
            interval,
            last_at: None,
            last_value: None,
        }
    }

    /// 보낼 값이면 Some(값)
    pub(crate) fn offer(&mut self, now: Instant, value: Option<f64>) -> Option<Option<f64>> {
        let value = value.map(round_progress);
        if self.last_value == Some(value) {
            return None;
        }
        if let Some(at) = self.last_at {
            if now.saturating_duration_since(at) < self.interval {
                return None;
            }
        }
        self.last_at = Some(now);
        self.last_value = Some(value);
        Some(value)
    }
}

// ===== 큐 =====

type Planner = dyn Fn(&EngineSet, &PlanInput<'_>) -> Result<Vec<Step>, CoreError> + Send + Sync;

/// 끝난 배치 요약을 몇 개까지 남길지 (wait_batch용)
const KEEP_FINISHED_BATCHES: usize = 256;
/// ffprobe 제한 시간
const DURATION_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
struct PendingJob {
    job_id: JobId,
    batch_id: BatchId,
    input: PathBuf,
    input_format: &'static str,
    to: &'static str,
    engines: &'static [EngineId],
    preset: PresetId,
    out_dir: Option<PathBuf>,
    overwrite: bool,
    cancel: CancelToken,
}

impl PendingJob {
    fn uses_office(&self) -> bool {
        self.engines.contains(&EngineId::Office)
    }
}

#[derive(Debug)]
struct BatchState {
    remaining: usize,
    outcomes: Vec<JobOutcome>,
    rejected: Vec<RejectedFile>,
    summary: Option<BatchSummary>,
}

#[derive(Default)]
struct State {
    next_job: JobId,
    next_batch: BatchId,
    pending: VecDeque<PendingJob>,
    running: usize,
    running_office: usize,
    /// 끝나지 않은 모든 작업(대기·실행 중)의 취소 토큰
    tokens: HashMap<JobId, (BatchId, CancelToken)>,
    batches: HashMap<BatchId, BatchState>,
    finished_batches: VecDeque<BatchId>,
    shutdown: bool,
}

struct Inner {
    engines: Arc<EngineSet>,
    config: QueueConfig,
    sink: EventSink,
    planner: Box<Planner>,
    state: Mutex<State>,
    work_cv: Condvar,
    done_cv: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 작업 큐. std 스레드 + Mutex/Condvar로 구현한다(tokio 쓰지 않음).
///
/// `Send + Sync`여야 한다(앱이 Tauri 상태로 공유한다).
pub struct JobQueue {
    inner: Arc<Inner>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl JobQueue {
    pub fn new(engines: Arc<EngineSet>, config: QueueConfig, sink: EventSink) -> Self {
        Self::with_planner(engines, config, sink, Box::new(engines::plan))
    }

    /// 단계 계획 함수를 바꿔 끼운다(테스트용: 가짜 엔진 명령).
    pub(crate) fn with_planner(
        engines: Arc<EngineSet>,
        config: QueueConfig,
        sink: EventSink,
        planner: Box<Planner>,
    ) -> Self {
        let workers = config.max_parallel.max(1);
        let inner = Arc::new(Inner {
            engines,
            config,
            sink,
            planner,
            state: Mutex::new(State::default()),
            work_cv: Condvar::new(),
            done_cv: Condvar::new(),
        });
        let handles = (0..workers)
            .filter_map(|i| {
                let inner = Arc::clone(&inner);
                std::thread::Builder::new()
                    .name(format!("convtool-job-{i}"))
                    .spawn(move || worker_loop(inner))
                    .map_err(|e| tracing::error!("작업 스레드를 만들지 못했습니다: {e}"))
                    .ok()
            })
            .collect();
        JobQueue {
            inner,
            workers: Mutex::new(handles),
        }
    }

    /// 파일마다 미리 판정: 존재하지 않음→invalid_input, 형식 불가→unsupported_format,
    /// 엔진 없음→engine_missing, 인코더 없음→codec_missing은 rejected로. 나머지는 큐에 넣는다.
    /// queued/rejected 이벤트를 sink로 보낸다. 실행할 작업이 0개면 batch_done을 바로 보낸다.
    /// Err는 요청 자체가 잘못될 때만: files 비었음 → invalid_input,
    /// to가 형식 표의 출력 대상이 아님 → unsupported_format(spec 7장 종료 코드 2).
    /// out_dir를 만들지 못하면 Err(Io), 큐가 이미 종료됐으면 Err(Internal).
    pub fn submit(&self, request: JobRequest) -> Result<BatchTicket, CoreError> {
        if request.files.is_empty() {
            return Err(CoreError::InvalidInput("변환할 파일이 없습니다".into()));
        }
        let to = formats::normalize_ext(&request.to)
            .filter(|t| formats::is_known_output(t))
            .ok_or_else(|| CoreError::UnsupportedFormat {
                input: input_exts_label(&request.files),
                to: Some(request.to.clone()),
            })?;
        if let Some(dir) = &request.out_dir {
            std::fs::create_dir_all(dir).map_err(|e| CoreError::Io {
                path: dir.clone(),
                source: e,
            })?;
        }

        let checks: Vec<(PathBuf, Precheck)> = request
            .files
            .iter()
            .map(|f| (f.clone(), precheck(&self.inner.engines, f, to)))
            .collect();

        // 번호 예약
        let mut jobs: Vec<PendingJob> = Vec::new();
        let mut rejected: Vec<RejectedFile> = Vec::new();
        // 이벤트 순서 = 파일 순서
        let mut events: Vec<JobEvent> = Vec::new();
        let batch_id = {
            let mut st = lock(&self.inner.state);
            if st.shutdown {
                return Err(CoreError::Internal("작업 큐가 종료되었습니다".into()));
            }
            st.next_batch += 1;
            let batch_id = st.next_batch;
            for (input, check) in checks {
                match check {
                    Ok((input_format, engines)) => {
                        st.next_job += 1;
                        let job_id = st.next_job;
                        let cancel = CancelToken::new();
                        st.tokens.insert(job_id, (batch_id, cancel.clone()));
                        events.push(JobEvent::Queued {
                            batch_id,
                            job_id,
                            input: input.clone(),
                            to: to.to_string(),
                        });
                        jobs.push(PendingJob {
                            job_id,
                            batch_id,
                            input,
                            input_format,
                            to,
                            engines,
                            preset: request.preset,
                            out_dir: request.out_dir.clone(),
                            overwrite: request.overwrite,
                            cancel,
                        });
                    }
                    Err(e) => {
                        let error = e.info();
                        events.push(JobEvent::Rejected {
                            batch_id,
                            input: input.clone(),
                            error: error.clone(),
                        });
                        rejected.push(RejectedFile { input, error });
                    }
                }
            }
            st.batches.insert(
                batch_id,
                BatchState {
                    remaining: jobs.len(),
                    outcomes: Vec::new(),
                    rejected: rejected.clone(),
                    summary: None,
                },
            );
            batch_id
        };

        for ev in &events {
            (self.inner.sink)(ev);
        }

        let ticket = BatchTicket {
            batch_id,
            jobs: jobs
                .iter()
                .map(|j| QueuedJob {
                    job_id: j.job_id,
                    input: j.input.clone(),
                    to: j.to.to_string(),
                })
                .collect(),
            rejected: rejected.clone(),
        };

        if jobs.is_empty() {
            self.inner.complete_batch(BatchSummary {
                batch_id,
                outcomes: Vec::new(),
                rejected,
            });
            return Ok(ticket);
        }

        let late = {
            let mut st = lock(&self.inner.state);
            if st.shutdown {
                // 예약과 넣기 사이에 종료됐다: 실행하지 않고 취소로 끝낸다.
                jobs
            } else {
                st.pending.extend(jobs);
                Vec::new()
            }
        };
        self.inner.work_cv.notify_all();
        for j in late {
            self.inner
                .finish(j.batch_id, j.job_id, j.input, OutcomeKind::Canceled);
        }
        Ok(ticket)
    }

    /// 조건에 맞는 끝나지 않은 작업을 취소한다. 대기 중이면 바로 canceled로 끝내고,
    /// 실행 중이면 토큰을 세워 엔진 프로세스를 죽이게 한다. 맞은 작업 수를 돌려준다.
    fn cancel_where(&self, pred: impl Fn(JobId, BatchId) -> bool) -> usize {
        let (count, removed) = {
            let mut st = lock(&self.inner.state);
            let mut count = 0;
            for (&job_id, (batch_id, token)) in &st.tokens {
                if pred(job_id, *batch_id) {
                    token.cancel();
                    count += 1;
                }
            }
            let mut removed = Vec::new();
            let mut keep = VecDeque::with_capacity(st.pending.len());
            for j in st.pending.drain(..) {
                if pred(j.job_id, j.batch_id) {
                    removed.push(j);
                } else {
                    keep.push_back(j);
                }
            }
            st.pending = keep;
            (count, removed)
        };
        for j in removed {
            self.inner
                .finish(j.batch_id, j.job_id, j.input, OutcomeKind::Canceled);
        }
        count
    }

    pub fn cancel(&self, job_id: JobId) -> bool {
        self.cancel_where(|j, _| j == job_id) > 0
    }

    pub fn cancel_batch(&self, batch_id: BatchId) -> usize {
        self.cancel_where(|_, b| b == batch_id)
    }

    pub fn cancel_all(&self) {
        self.cancel_where(|_, _| true);
    }

    /// batch_done이 날 때까지 막는다 (CLI용). 모르는 batch_id면 None.
    /// batch_done 이벤트를 sink로 보낸 뒤에 돌아온다.
    pub fn wait_batch(&self, batch_id: BatchId) -> Option<BatchSummary> {
        let mut st = lock(&self.inner.state);
        loop {
            match st.batches.get(&batch_id) {
                None => return None,
                Some(BatchState {
                    summary: Some(s), ..
                }) => return Some(s.clone()),
                Some(_) => {}
            }
            st = self
                .inner
                .done_cv
                .wait(st)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    /// 끝나지 않은 작업 수 (대기 + 실행 중)
    pub fn active_count(&self) -> usize {
        lock(&self.inner.state).tokens.len()
    }

    /// 모두 취소하고 작업 스레드를 join한다. Drop에서도 부른다. 여러 번 불러도 된다.
    pub fn shutdown(&self) {
        let drained: Vec<PendingJob> = {
            let mut st = lock(&self.inner.state);
            st.shutdown = true;
            for (_, token) in st.tokens.values() {
                token.cancel();
            }
            st.pending.drain(..).collect()
        };
        self.inner.work_cv.notify_all();
        for j in drained {
            self.inner
                .finish(j.batch_id, j.job_id, j.input, OutcomeKind::Canceled);
        }
        let handles = std::mem::take(&mut *lock(&self.workers));
        let me = std::thread::current().id();
        for h in handles {
            if h.thread().id() != me {
                let _ = h.join();
            }
        }
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 오류 메시지용 입력 형식 표시: 파일 확장자(소문자)를 중복 없이 처음 나온 순서로 ", "로 잇는다.
/// 확장자가 없으면 "?".
fn input_exts_label(files: &[PathBuf]) -> String {
    let mut exts: Vec<String> = Vec::new();
    for f in files {
        let ext = f
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "?".into());
        if !exts.contains(&ext) {
            exts.push(ext);
        }
    }
    exts.join(", ")
}

/// 파일별 판정 결과: (정규화된 입력 형식, 사용할 엔진) 또는 거절 사유
type Precheck = Result<(&'static str, &'static [EngineId]), CoreError>;

/// 요청 단계의 파일별 판정
fn precheck(engines: &EngineSet, input: &Path, to: &'static str) -> Precheck {
    if !input.is_file() {
        return Err(CoreError::InvalidInput(format!(
            "파일이 없습니다: {}",
            input.display()
        )));
    }
    let Some(input_format) = formats::format_of_path(input) else {
        let ext = input
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "?".into());
        return Err(CoreError::UnsupportedFormat {
            input: ext,
            to: Some(to.to_string()),
        });
    };
    let entry =
        formats::output_entry(input_format, to).ok_or_else(|| CoreError::UnsupportedFormat {
            input: input_format.to_string(),
            to: Some(to.to_string()),
        })?;
    if let Some(problem) = formats::availability_problem(&[input_format], entry, engines) {
        return Err(problem.error);
    }
    Ok((input_format, entry.engines))
}

impl Inner {
    fn emit(&self, ev: JobEvent) {
        (self.sink)(&ev);
    }

    /// 작업 하나를 끝낸다: 끝 이벤트를 보내고 결과를 배치에 적는다. 배치의 마지막이면 batch_done.
    fn finish(&self, batch_id: BatchId, job_id: JobId, input: PathBuf, outcome: OutcomeKind) {
        self.emit(match &outcome {
            OutcomeKind::Done { output } => JobEvent::Done {
                batch_id,
                job_id,
                input: input.clone(),
                output: output.clone(),
            },
            OutcomeKind::Failed { error } => JobEvent::Failed {
                batch_id,
                job_id,
                input: input.clone(),
                error: error.clone(),
            },
            OutcomeKind::Canceled => JobEvent::Canceled {
                batch_id,
                job_id,
                input: input.clone(),
            },
        });
        let summary = {
            let mut st = lock(&self.state);
            st.tokens.remove(&job_id);
            match st.batches.get_mut(&batch_id) {
                Some(b) => {
                    b.outcomes.push(JobOutcome {
                        job_id,
                        input,
                        outcome,
                    });
                    b.remaining = b.remaining.saturating_sub(1);
                    if b.remaining == 0 {
                        let mut outcomes = b.outcomes.clone();
                        outcomes.sort_by_key(|o| o.job_id);
                        Some(BatchSummary {
                            batch_id,
                            outcomes,
                            rejected: b.rejected.clone(),
                        })
                    } else {
                        None
                    }
                }
                None => None,
            }
        };
        if let Some(s) = summary {
            self.complete_batch(s);
        }
    }

    /// batch_done을 보내고 요약을 기록한 뒤 wait_batch를 깨운다.
    fn complete_batch(&self, summary: BatchSummary) {
        self.emit(summary.batch_done_event());
        {
            let mut st = lock(&self.state);
            let id = summary.batch_id;
            if let Some(b) = st.batches.get_mut(&id) {
                b.summary = Some(summary);
            }
            st.finished_batches.push_back(id);
            while st.finished_batches.len() > KEEP_FINISHED_BATCHES {
                if let Some(old) = st.finished_batches.pop_front() {
                    st.batches.remove(&old);
                }
            }
        }
        self.done_cv.notify_all();
    }
}

fn worker_loop(inner: Arc<Inner>) {
    loop {
        let job = {
            let mut st = lock(&inner.state);
            loop {
                if st.shutdown && st.pending.is_empty() {
                    return;
                }
                let flags: Vec<bool> = st.pending.iter().map(PendingJob::uses_office).collect();
                let pick = pick_next(
                    &flags,
                    st.running,
                    st.running_office,
                    inner.config.max_parallel,
                    inner.config.office_parallel,
                );
                if let Some(job) = pick.and_then(|i| st.pending.remove(i)) {
                    st.running += 1;
                    if job.uses_office() {
                        st.running_office += 1;
                    }
                    break job;
                }
                st = inner.work_cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };

        let outcome = if job.cancel.is_canceled() {
            OutcomeKind::Canceled
        } else {
            execute(&inner, &job)
        };

        {
            let mut st = lock(&inner.state);
            st.running = st.running.saturating_sub(1);
            if job.uses_office() {
                st.running_office = st.running_office.saturating_sub(1);
            }
        }
        inner.work_cv.notify_all();
        inner.finish(job.batch_id, job.job_id, job.input.clone(), outcome);
    }
}

fn execute(inner: &Inner, job: &PendingJob) -> OutcomeKind {
    match run_job(inner, job) {
        Ok(output) => OutcomeKind::Done { output },
        Err(CoreError::Canceled) => OutcomeKind::Canceled,
        Err(_) if job.cancel.is_canceled() => OutcomeKind::Canceled,
        Err(e) => {
            tracing::warn!(job_id = job.job_id, input = %job.input.display(), "변환 실패: {e}");
            OutcomeKind::Failed { error: e.info() }
        }
    }
}

/// 작업 폴더와 남은 임시 파일을 지운다(성공·실패·취소 모두).
struct Cleanup {
    work_dir: PathBuf,
    tmp: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.work_dir);
        if self.tmp.exists() {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

/// 이름 바꾸기, 안 되면(다른 드라이브) 복사 후 삭제
fn move_file(from: &Path, to: &Path) -> Result<(), CoreError> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| CoreError::Io {
        path: to.to_path_buf(),
        source: e,
    })?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

fn probe_duration(engines: &EngineSet, input: &Path) -> Option<f64> {
    let ffprobe = engines.ffprobe_path()?;
    let spec = ffmpeg::probe_duration_command(ffprobe, input);
    match capture_stdout(&spec, DURATION_PROBE_TIMEOUT) {
        Ok(out) => ffmpeg::parse_duration(&out),
        Err(e) => {
            tracing::debug!("ffprobe 실패: {e}");
            None
        }
    }
}

/// 작업 하나의 실행 순서 (설계 계획 "작업 하나의 실행 순서" 1~7)
fn run_job(inner: &Inner, job: &PendingJob) -> Result<PathBuf, CoreError> {
    let desired = naming::desired_output(&job.input, job.out_dir.as_deref(), job.to);
    let final_dir = desired.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = job
        .input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    let tmp = naming::temp_output(&final_dir, &stem, job.job_id, job.to);
    let work_dir = std::env::temp_dir().join("convtool").join(format!(
        "{}-{}",
        std::process::id(),
        job.job_id
    ));
    let _cleanup = Cleanup {
        work_dir: work_dir.clone(),
        tmp: tmp.clone(),
    };
    std::fs::create_dir_all(&work_dir).map_err(|e| CoreError::Io {
        path: work_dir.clone(),
        source: e,
    })?;

    let duration = if job.engines.contains(&EngineId::Ffmpeg) {
        probe_duration(&inner.engines, &job.input)
    } else {
        None
    };

    let steps = (inner.planner)(
        &inner.engines,
        &PlanInput {
            input: &job.input,
            input_format: job.input_format,
            to: job.to,
            preset: job.preset,
            tmp_output: &tmp,
            work_dir: &work_dir,
            duration_secs: duration,
        },
    )?;
    let last_output = steps
        .last()
        .map(|s| s.output.clone())
        .ok_or_else(|| CoreError::Internal("실행할 단계가 없습니다".into()))?;

    let mut throttle = ProgressThrottle::new(inner.config.progress_interval);
    let mut base = 0.0_f64;
    for step in &steps {
        if job.cancel.is_canceled() {
            return Err(CoreError::Canceled);
        }
        inner.emit(JobEvent::Started {
            batch_id: job.batch_id,
            job_id: job.job_id,
            engine: step.engine,
        });
        let weight = step.weight;
        let step_duration = if step.engine == EngineId::Ffmpeg {
            duration
        } else {
            None
        };
        run_step_with(step, step_duration, &job.cancel, &mut |p| {
            // 두 번째 단계부터는 비율을 몰라도 앞 단계까지의 진행률을 유지한다.
            let overall = match p {
                Some(f) => Some(base + weight * f),
                None if base > 0.0 => Some(base),
                None => None,
            };
            if let Some(v) = throttle.offer(Instant::now(), overall) {
                inner.emit(JobEvent::Progress {
                    batch_id: job.batch_id,
                    job_id: job.job_id,
                    progress: v,
                });
            }
        })?;
        if !step.output.is_file() {
            return Err(CoreError::EngineFailed {
                engine: step.engine,
                code: Some(0),
                stderr_tail: format!(
                    "출력 파일이 만들어지지 않았습니다: {}",
                    step.output.display()
                ),
            });
        }
        base += weight;
    }

    if last_output != tmp {
        move_file(&last_output, &tmp)?;
    }
    if job.cancel.is_canceled() {
        return Err(CoreError::Canceled);
    }
    naming::commit(&tmp, &desired, &job.input, job.overwrite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// 설계 계획 (4) CLI `--json` 예시 줄
    const CLI_LINES: [&str; 6] = [
        r#"{"type":"queued","batch_id":1,"job_id":1,"input":"C:\\영상\\발표 녹화.mov","to":"mp4"}"#,
        r#"{"type":"rejected","batch_id":1,"input":"C:\\영상\\메모.xyz","error":{"code":"unsupported_format","message":"지원하지 않는 형식입니다: xyz → mp4","engine":null}}"#,
        r#"{"type":"started","batch_id":1,"job_id":1,"engine":"ffmpeg"}"#,
        r#"{"type":"progress","batch_id":1,"job_id":1,"progress":0.42}"#,
        r#"{"type":"done","batch_id":1,"job_id":1,"input":"C:\\영상\\발표 녹화.mov","output":"C:\\영상\\발표 녹화.mp4"}"#,
        r#"{"type":"batch_done","batch_id":1,"succeeded":1,"failed":0,"canceled":0,"rejected":1,"exit_code":1}"#,
    ];

    #[test]
    fn cli_json_lines_round_trip() {
        for line in CLI_LINES {
            let ev: JobEvent = serde_json::from_str(line).expect(line);
            let out = serde_json::to_string(&ev).expect("직렬화");
            assert!(!out.contains('\n'));
            let a: Value = serde_json::from_str(line).expect("원본");
            let b: Value = serde_json::from_str(&out).expect("재직렬화");
            assert_eq!(a, b, "{line}");
        }
    }

    #[test]
    fn cli_json_line_exact_from_values() {
        let ev = JobEvent::Rejected {
            batch_id: 1,
            input: PathBuf::from(r"C:\영상\메모.xyz"),
            error: CoreError::UnsupportedFormat {
                input: "xyz".into(),
                to: Some("mp4".into()),
            }
            .info(),
        };
        let out = serde_json::to_string(&ev).expect("직렬화");
        assert_eq!(out, CLI_LINES[1]);

        let ev = JobEvent::Progress {
            batch_id: 1,
            job_id: 1,
            progress: None,
        };
        assert_eq!(
            serde_json::to_value(&ev).expect("직렬화"),
            json!({"type":"progress","batch_id":1,"job_id":1,"progress":null})
        );
    }

    #[test]
    fn job_request_defaults_and_nulls() {
        let r: JobRequest =
            serde_json::from_str(r#"{"files":["a.mov"],"to":"mp4"}"#).expect("기본값");
        assert_eq!(r.preset, PresetId::Web);
        assert_eq!(r.out_dir, None);
        assert!(!r.overwrite);
        assert_eq!(
            serde_json::to_value(&r).expect("직렬화"),
            json!({"files":["a.mov"],"to":"mp4","preset":"web","out_dir":null,"overwrite":false})
        );
    }

    #[test]
    fn outcome_kind_tag() {
        let o = JobOutcome {
            job_id: 2,
            input: PathBuf::from("a.mov"),
            outcome: OutcomeKind::Canceled,
        };
        assert_eq!(
            serde_json::to_value(&o).expect("직렬화"),
            json!({"job_id":2,"input":"a.mov","outcome":{"kind":"canceled"}})
        );
        let d = OutcomeKind::Done {
            output: PathBuf::from("a.mp4"),
        };
        assert_eq!(
            serde_json::to_value(&d).expect("직렬화"),
            json!({"kind":"done","output":"a.mp4"})
        );
    }

    fn rejected(code: ErrorCode) -> RejectedFile {
        RejectedFile {
            input: PathBuf::from("x"),
            error: ErrorInfo {
                code,
                message: String::new(),
                engine: None,
            },
        }
    }

    fn outcome(kind: OutcomeKind) -> JobOutcome {
        JobOutcome {
            job_id: 1,
            input: PathBuf::from("a"),
            outcome: kind,
        }
    }

    fn done() -> OutcomeKind {
        OutcomeKind::Done {
            output: PathBuf::from("b"),
        }
    }

    #[test]
    fn batch_summary_exit_codes() {
        let s = |outcomes: Vec<JobOutcome>, rejected: Vec<RejectedFile>| BatchSummary {
            batch_id: 1,
            outcomes,
            rejected,
        };
        assert_eq!(s(vec![], vec![]).exit_code(), 0);
        assert_eq!(s(vec![outcome(done())], vec![]).exit_code(), 0);
        assert_eq!(
            s(vec![], vec![rejected(ErrorCode::UnsupportedFormat)]).exit_code(),
            2
        );
        assert_eq!(
            s(vec![], vec![rejected(ErrorCode::InvalidInput)]).exit_code(),
            2
        );
        assert_eq!(
            s(
                vec![],
                vec![
                    rejected(ErrorCode::UnsupportedFormat),
                    rejected(ErrorCode::EngineMissing)
                ]
            )
            .exit_code(),
            3
        );
        assert_eq!(
            s(vec![], vec![rejected(ErrorCode::CodecMissing)]).exit_code(),
            3
        );
        assert_eq!(
            s(
                vec![outcome(done())],
                vec![rejected(ErrorCode::EngineMissing)]
            )
            .exit_code(),
            1
        );
        let failed = OutcomeKind::Failed {
            error: CoreError::Internal("x".into()).info(),
        };
        assert_eq!(s(vec![outcome(failed.clone())], vec![]).exit_code(), 1);
        assert_eq!(
            s(
                vec![outcome(done()), outcome(OutcomeKind::Canceled)],
                vec![]
            )
            .exit_code(),
            1
        );
        assert_eq!(
            s(vec![outcome(OutcomeKind::Canceled)], vec![]).exit_code(),
            1
        );

        let ev = s(
            vec![
                outcome(done()),
                outcome(failed),
                outcome(OutcomeKind::Canceled),
            ],
            vec![rejected(ErrorCode::Io)],
        )
        .batch_done_event();
        assert_eq!(
            ev,
            JobEvent::BatchDone {
                batch_id: 1,
                succeeded: 1,
                failed: 1,
                canceled: 1,
                rejected: 1,
                exit_code: 1
            }
        );
    }

    #[test]
    fn queue_config_default() {
        let c = QueueConfig::default();
        assert_eq!(c.max_parallel, 2);
        assert_eq!(c.office_parallel, 1);
        assert_eq!(c.progress_interval, Duration::from_millis(100));
    }

    #[test]
    fn job_queue_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<JobQueue>();
    }

    // ----- 스케줄러 -----

    #[test]
    fn scheduler_limits() {
        // 전체 제한
        assert_eq!(pick_next(&[false], 2, 0, 2, 1), None);
        assert_eq!(pick_next(&[false], 1, 0, 2, 1), Some(0));
        assert_eq!(pick_next(&[], 0, 0, 2, 1), None);
        // office 1개 실행 중이면 다음 office는 건너뛰고 일반 작업을 먼저
        assert_eq!(pick_next(&[true, true, false], 1, 1, 2, 1), Some(2));
        assert_eq!(pick_next(&[true, true], 1, 1, 2, 1), None);
        // office가 없으면 FIFO
        assert_eq!(pick_next(&[true, false], 0, 0, 2, 1), Some(0));
        // office_parallel 2
        assert_eq!(pick_next(&[true], 1, 1, 3, 2), Some(0));
        // 0은 1로 본다
        assert_eq!(pick_next(&[true], 0, 0, 0, 0), Some(0));
        assert_eq!(pick_next(&[false], 1, 0, 0, 0), None);
    }

    #[test]
    fn progress_throttle() {
        let t0 = Instant::now();
        let mut th = ProgressThrottle::new(Duration::from_millis(100));
        assert_eq!(th.offer(t0, None), Some(None));
        assert_eq!(th.offer(t0 + Duration::from_millis(10), Some(0.1)), None);
        assert_eq!(
            th.offer(t0 + Duration::from_millis(100), Some(0.41666)),
            Some(Some(0.417))
        );
        // 같은 값은 다시 보내지 않는다
        assert_eq!(
            th.offer(t0 + Duration::from_millis(300), Some(0.4169)),
            None
        );
        assert_eq!(
            th.offer(t0 + Duration::from_millis(400), Some(1.7)),
            Some(Some(1.0))
        );
        assert_eq!(round_progress(-0.2), 0.0);
        assert_eq!(round_progress(0.4245), 0.425);
    }

    // ----- 큐 (엔진 없이) -----

    fn collector() -> (EventSink, Arc<Mutex<Vec<JobEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let e2 = Arc::clone(&events);
        let sink: EventSink = Arc::new(move |ev: &JobEvent| lock(&e2).push(ev.clone()));
        (sink, events)
    }

    fn all_engines() -> Arc<EngineSet> {
        let paths = EngineId::ALL
            .iter()
            .map(|&id| (id, PathBuf::from(format!("{id}.exe"))))
            .collect();
        Arc::new(EngineSet::from_parts(paths, None, None))
    }

    #[test]
    fn submit_request_errors() {
        let (sink, events) = collector();
        let q = JobQueue::new(all_engines(), QueueConfig::default(), sink);
        let req = |files: Vec<PathBuf>, to: &str| JobRequest {
            files,
            to: to.into(),
            preset: PresetId::Web,
            out_dir: None,
            overwrite: false,
        };
        let e = q.submit(req(vec![], "mp4")).expect_err("빈 목록");
        assert_eq!(e.code(), ErrorCode::InvalidInput);
        let e = q
            .submit(req(vec!["a.mov".into(), "b.MOV".into(), "c".into()], "xyz"))
            .expect_err("알 수 없는 형식");
        assert_eq!(e.code(), ErrorCode::UnsupportedFormat);
        assert_eq!(e.code().cli_exit_code(), 2);
        assert_eq!(e.info().message, "지원하지 않는 형식입니다: mov, ? → xyz");
        assert_eq!(e.info().engine, None);
        let e = q
            .submit(req(vec!["a.mov".into()], "mov"))
            .expect_err("출력 아님");
        assert_eq!(e.code(), ErrorCode::UnsupportedFormat);
        assert!(lock(&events).is_empty());
        assert_eq!(q.wait_batch(999), None);
    }

    #[test]
    fn submit_all_rejected_finishes_immediately() {
        let dir = tempfile::tempdir().expect("임시 폴더");
        let mp4 = dir.path().join("발표 녹화.mp4");
        let mov = dir.path().join("발표 녹화.mov");
        let xyz = dir.path().join("메모.xyz");
        for p in [&mp4, &mov, &xyz] {
            std::fs::write(p, b"x").expect("쓰기");
        }
        let (sink, events) = collector();
        // ffmpeg 없음
        let q = JobQueue::new(Arc::new(EngineSet::default()), QueueConfig::default(), sink);
        let t = q
            .submit(JobRequest {
                files: vec![mp4.clone(), xyz.clone(), dir.path().join("없음.mov")],
                to: "mp4".into(),
                preset: PresetId::Web,
                out_dir: None,
                overwrite: false,
            })
            .expect("요청");
        assert!(t.jobs.is_empty());
        let codes: Vec<ErrorCode> = t.rejected.iter().map(|r| r.error.code).collect();
        assert_eq!(
            codes,
            [
                ErrorCode::UnsupportedFormat,
                ErrorCode::UnsupportedFormat,
                ErrorCode::InvalidInput
            ]
        );
        assert_eq!(
            t.rejected[1].error.message,
            "지원하지 않는 형식입니다: xyz → mp4"
        );
        let s = q.wait_batch(t.batch_id).expect("요약");
        assert_eq!(s.exit_code(), 2);

        let t = q
            .submit(JobRequest {
                files: vec![mov.clone()],
                to: "webm".into(),
                preset: PresetId::Web,
                out_dir: None,
                overwrite: false,
            })
            .expect("요청");
        assert_eq!(t.rejected[0].error.code, ErrorCode::EngineMissing);
        assert_eq!(t.rejected[0].error.engine, Some(EngineId::Ffmpeg));
        assert_eq!(q.wait_batch(t.batch_id).expect("요약").exit_code(), 3);

        let ev = lock(&events).clone();
        assert!(matches!(
            ev.last(),
            Some(JobEvent::BatchDone { exit_code: 3, .. })
        ));
        assert_eq!(
            ev.iter()
                .filter(|e| matches!(e, JobEvent::BatchDone { .. }))
                .count(),
            2
        );
    }

    #[cfg(windows)]
    mod with_cmd {
        use super::*;
        use crate::engines::{CommandSpec, ProgressStream};
        use std::ffi::OsString;

        fn cmd_exe() -> PathBuf {
            let root =
                std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
            PathBuf::from(root).join("System32").join("cmd.exe")
        }

        /// tmp_output에 무엇을 쓸지 정하는 가짜 계획
        fn fake_planner(
            script: impl Fn(&PlanInput<'_>) -> Vec<OsString> + Send + Sync + 'static,
        ) -> Box<Planner> {
            Box::new(move |_engines, p| {
                Ok(vec![Step {
                    engine: EngineId::Ffmpeg,
                    command: CommandSpec {
                        program: cmd_exe(),
                        args: script(p),
                        cwd: None,
                        progress: ProgressStream::None,
                    },
                    output: p.tmp_output.to_path_buf(),
                    weight: 1.0,
                }])
            })
        }

        fn copy_planner() -> Box<Planner> {
            fake_planner(|p| {
                vec![
                    "/C".into(),
                    "copy".into(),
                    "/Y".into(),
                    p.input.as_os_str().to_os_string(),
                    p.tmp_output.as_os_str().to_os_string(),
                ]
            })
        }

        fn slow_planner() -> Box<Planner> {
            fake_planner(|_| vec!["/C".into(), "ping -n 30 127.0.0.1 >NUL".into()])
        }

        fn req(files: Vec<PathBuf>) -> JobRequest {
            JobRequest {
                files,
                to: "mp4".into(),
                preset: PresetId::Web,
                out_dir: None,
                overwrite: false,
            }
        }

        fn no_temp_files(dir: &Path) -> bool {
            std::fs::read_dir(dir)
                .expect("폴더 읽기")
                .filter_map(Result::ok)
                .all(|e| !e.file_name().to_string_lossy().contains("convtool-tmp"))
        }

        fn wait_for(events: &Arc<Mutex<Vec<JobEvent>>>, pred: impl Fn(&JobEvent) -> bool) {
            let start = Instant::now();
            while !lock(events).iter().any(&pred) {
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "이벤트 대기 시간 초과"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        #[test]
        fn success_with_conflict_and_rejections() {
            let dir = tempfile::tempdir().expect("임시 폴더");
            let input = dir.path().join("발표 녹화.mov");
            std::fs::write(&input, "원본").expect("쓰기");
            std::fs::write(dir.path().join("발표 녹화.mp4"), "기존").expect("쓰기");
            let xyz = dir.path().join("메모.xyz");
            std::fs::write(&xyz, "x").expect("쓰기");

            let (sink, events) = collector();
            let q =
                JobQueue::with_planner(all_engines(), QueueConfig::default(), sink, copy_planner());
            let t = q
                .submit(req(vec![input.clone(), xyz.clone()]))
                .expect("요청");
            assert_eq!(t.jobs.len(), 1);
            assert_eq!(t.rejected.len(), 1);
            let s = q.wait_batch(t.batch_id).expect("요약");
            let out = dir.path().join("발표 녹화 (1).mp4");
            assert_eq!(
                s.outcomes[0].outcome,
                OutcomeKind::Done {
                    output: out.clone()
                }
            );
            assert_eq!(s.exit_code(), 1);
            assert_eq!(std::fs::read_to_string(&out).expect("결과"), "원본");
            assert_eq!(
                std::fs::read_to_string(dir.path().join("발표 녹화.mp4")).expect("기존"),
                "기존"
            );
            assert!(no_temp_files(dir.path()));
            let work = std::env::temp_dir().join("convtool").join(format!(
                "{}-{}",
                std::process::id(),
                t.jobs[0].job_id
            ));
            assert!(!work.exists(), "작업 폴더가 지워져야 한다");

            let ev = lock(&events).clone();
            let kinds: Vec<&str> = ev
                .iter()
                .map(|e| match e {
                    JobEvent::Queued { .. } => "queued",
                    JobEvent::Rejected { .. } => "rejected",
                    JobEvent::Started { .. } => "started",
                    JobEvent::Progress { .. } => "progress",
                    JobEvent::Done { .. } => "done",
                    JobEvent::Failed { .. } => "failed",
                    JobEvent::Canceled { .. } => "canceled",
                    JobEvent::BatchDone { .. } => "batch_done",
                })
                .filter(|k| *k != "progress")
                .collect();
            assert_eq!(
                kinds,
                ["queued", "rejected", "started", "done", "batch_done"]
            );
            assert_eq!(
                ev.last(),
                Some(&JobEvent::BatchDone {
                    batch_id: t.batch_id,
                    succeeded: 1,
                    failed: 0,
                    canceled: 0,
                    rejected: 1,
                    exit_code: 1
                })
            );
            assert_eq!(q.active_count(), 0);
        }

        #[test]
        fn out_dir_and_overwrite() {
            let dir = tempfile::tempdir().expect("임시 폴더");
            let input = dir.path().join("a b.mov");
            std::fs::write(&input, "원본").expect("쓰기");
            let out_dir = dir.path().join("새 폴더").join("출력");
            std::fs::create_dir_all(&out_dir).expect("폴더");
            std::fs::write(out_dir.join("a b.mp4"), "기존").expect("쓰기");
            let (sink, _events) = collector();
            let q =
                JobQueue::with_planner(all_engines(), QueueConfig::default(), sink, copy_planner());
            let mut r = req(vec![input.clone()]);
            r.out_dir = Some(out_dir.clone());
            r.overwrite = true;
            let t = q.submit(r).expect("요청");
            let s = q.wait_batch(t.batch_id).expect("요약");
            assert_eq!(s.exit_code(), 0);
            assert_eq!(
                std::fs::read_to_string(out_dir.join("a b.mp4")).expect("결과"),
                "원본"
            );
            assert!(no_temp_files(&out_dir));
        }

        #[test]
        fn engine_failure_and_missing_output() {
            let dir = tempfile::tempdir().expect("임시 폴더");
            let input = dir.path().join("a.mov");
            std::fs::write(&input, "원본").expect("쓰기");
            let (sink, _e) = collector();
            let q = JobQueue::with_planner(
                all_engines(),
                QueueConfig::default(),
                sink,
                fake_planner(|_| vec!["/C".into(), "echo engine-broke 1>&2& exit 5".into()]),
            );
            let t = q.submit(req(vec![input.clone()])).expect("요청");
            let s = q.wait_batch(t.batch_id).expect("요약");
            match &s.outcomes[0].outcome {
                OutcomeKind::Failed { error } => {
                    assert_eq!(error.code, ErrorCode::EngineFailed);
                    assert!(error.message.contains("engine-broke"), "{}", error.message);
                }
                other => panic!("예상 밖: {other:?}"),
            }
            assert_eq!(s.exit_code(), 1);

            let (sink, _e) = collector();
            let q = JobQueue::with_planner(
                all_engines(),
                QueueConfig::default(),
                sink,
                fake_planner(|_| vec!["/C".into(), "echo 아무것도 안 씀".into()]),
            );
            let t = q.submit(req(vec![input.clone()])).expect("요청");
            let s = q.wait_batch(t.batch_id).expect("요약");
            assert!(matches!(
                &s.outcomes[0].outcome,
                OutcomeKind::Failed { error } if error.message.contains("출력 파일")
            ));
            assert!(no_temp_files(dir.path()));
            assert!(!dir.path().join("a.mp4").exists());
        }

        #[test]
        fn cancel_running_and_pending() {
            let dir = tempfile::tempdir().expect("임시 폴더");
            let a = dir.path().join("a.mov");
            let b = dir.path().join("b.mov");
            std::fs::write(&a, "x").expect("쓰기");
            std::fs::write(&b, "x").expect("쓰기");
            let (sink, events) = collector();
            let cfg = QueueConfig {
                max_parallel: 1,
                ..QueueConfig::default()
            };
            let q = JobQueue::with_planner(all_engines(), cfg, sink, slow_planner());
            let t = q.submit(req(vec![a.clone(), b.clone()])).expect("요청");
            let (ja, jb) = (t.jobs[0].job_id, t.jobs[1].job_id);
            wait_for(
                &events,
                |e| matches!(e, JobEvent::Started { job_id, .. } if *job_id == ja),
            );
            assert_eq!(q.active_count(), 2);

            let start = Instant::now();
            // 대기 중인 작업은 바로 canceled
            assert!(q.cancel(jb));
            wait_for(
                &events,
                |e| matches!(e, JobEvent::Canceled { job_id, .. } if *job_id == jb),
            );
            // 실행 중인 작업은 프로세스를 죽인다
            assert!(q.cancel(ja));
            let s = q.wait_batch(t.batch_id).expect("요약");
            assert!(start.elapsed() < Duration::from_secs(10));
            assert!(s
                .outcomes
                .iter()
                .all(|o| o.outcome == OutcomeKind::Canceled));
            assert_eq!(s.exit_code(), 1);
            assert!(!q.cancel(ja), "끝난 작업");
            assert!(no_temp_files(dir.path()));
            // started가 한 번만 (b는 실행되지 않음)
            assert_eq!(
                lock(&events)
                    .iter()
                    .filter(|e| matches!(e, JobEvent::Started { .. }))
                    .count(),
                1
            );
        }

        #[test]
        fn cancel_batch_and_shutdown() {
            let dir = tempfile::tempdir().expect("임시 폴더");
            let files: Vec<PathBuf> = (0..3)
                .map(|i| {
                    let p = dir.path().join(format!("{i}.mov"));
                    std::fs::write(&p, "x").expect("쓰기");
                    p
                })
                .collect();
            let (sink, events) = collector();
            let q =
                JobQueue::with_planner(all_engines(), QueueConfig::default(), sink, slow_planner());
            let t1 = q.submit(req(files.clone())).expect("요청");
            let t2 = q.submit(req(files[..1].to_vec())).expect("요청");
            wait_for(&events, |e| matches!(e, JobEvent::Started { .. }));
            assert_eq!(q.cancel_batch(t1.batch_id), 3);
            let s1 = q.wait_batch(t1.batch_id).expect("요약");
            assert_eq!(s1.outcomes.len(), 3);
            let start = Instant::now();
            q.shutdown();
            assert!(start.elapsed() < Duration::from_secs(10));
            let s2 = q.wait_batch(t2.batch_id).expect("요약");
            assert!(matches!(s2.outcomes[0].outcome, OutcomeKind::Canceled));
            assert!(q.submit(req(files[..1].to_vec())).is_err(), "종료 뒤 요청");
            q.shutdown(); // 두 번 불러도 된다
        }

        #[test]
        fn office_jobs_run_one_at_a_time() {
            let dir = tempfile::tempdir().expect("임시 폴더");
            let files: Vec<PathBuf> = (0..2)
                .map(|i| {
                    let p = dir.path().join(format!("{i}.docx"));
                    std::fs::write(&p, "x").expect("쓰기");
                    p
                })
                .collect();
            let (sink, events) = collector();
            let q =
                JobQueue::with_planner(all_engines(), QueueConfig::default(), sink, slow_planner());
            let mut r = req(files);
            r.to = "pdf".into();
            let t = q.submit(r).expect("요청");
            wait_for(&events, |e| matches!(e, JobEvent::Started { .. }));
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(
                lock(&events)
                    .iter()
                    .filter(|e| matches!(e, JobEvent::Started { .. }))
                    .count(),
                1,
                "office 작업은 동시에 1개"
            );
            q.cancel_all();
            let s = q.wait_batch(t.batch_id).expect("요약");
            assert_eq!(s.outcomes.len(), 2);
        }
    }
}
