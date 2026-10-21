//! FileKit 변환 코어 (`convtool-core`).
//!
//! UI·Tauri·Win32 UI 코드를 넣지 않는다. 같은 코어를 설치형 앱, CLI, 나중의 웹 서버가 함께 쓴다.

pub mod engines;
pub mod error;
pub mod formats;
pub mod job;
pub mod naming;
pub mod pipe;
pub mod presets;

pub use engines::{EngineConfig, EngineId, EngineInfo, EngineSet, EngineSource};
pub use error::{CoreError, ErrorCode, ErrorInfo};
pub use formats::{
    query_formats, Category, FileFormat, FormatOption, FormatsReport, RadialLayout, RadialSlot,
    SkipReason, SkippedFile, Unavailable, UnavailableCode,
};
pub use job::{
    BatchId, BatchSummary, BatchTicket, EventSink, JobEvent, JobId, JobOutcome, JobQueue,
    JobRequest, OutcomeKind, QueueConfig, QueuedJob, RejectedFile,
};
pub use presets::{PresetId, PresetInfo};

/// 크레이트 버전 (`server_version` 등에 쓴다).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
