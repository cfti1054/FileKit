//! 알림 (F-05). 지금은 `tracing` 로그만 남긴다.
//!
//! TODO(F-05): Windows 토스트
//! - 발신자 이름: AUMID `com.filekit.convtool` (installer/hooks.nsh가 `HKCU\Software\Classes\AppUserModelId`에
//!   `DisplayName`을 등록). 프로세스 시작 시 `SetCurrentProcessExplicitAppUserModelID`.
//! - 진행률: 같은 알림을 갱신 (`ToastNotification.Tag`/`Group` + `NotificationData`).
//! - 완료: "폴더에서 보기" 버튼 (`explorer.exe /select,<output>`).
//! - 실패: 원인 한 줄 (`ErrorInfo.message`).

use std::path::Path;

use convtool_core::{BatchId, ErrorInfo, JobEvent, JobId};

/// 알림 받는 쪽. 기본 구현은 아무것도 하지 않는다.
pub trait Notifier: Send + Sync {
    fn job_started(&self, _batch_id: BatchId, _job_id: JobId) {}
    fn job_progress(&self, _job_id: JobId, _progress: Option<f64>) {}
    fn job_done(&self, _job_id: JobId, _input: &Path, _output: &Path) {}
    fn job_failed(&self, _job_id: JobId, _input: &Path, _error: &ErrorInfo) {}
    fn batch_done(&self, _batch_id: BatchId, _succeeded: u32, _failed: u32) {}
    /// 작업 큐에 들어가기 전 오류 (원형 메뉴에서 고른 형식을 넣지 못함, 지원 형식 없음 등)
    fn error(&self, _error: &ErrorInfo) {}
}

/// `JobEvent`를 알맞은 메서드로 나눈다.
pub fn dispatch(n: &dyn Notifier, ev: &JobEvent) {
    match ev {
        JobEvent::Started {
            batch_id, job_id, ..
        } => n.job_started(*batch_id, *job_id),
        JobEvent::Progress {
            job_id, progress, ..
        } => n.job_progress(*job_id, *progress),
        JobEvent::Done {
            job_id,
            input,
            output,
            ..
        } => n.job_done(*job_id, input, output),
        JobEvent::Failed {
            job_id,
            input,
            error,
            ..
        } => n.job_failed(*job_id, input, error),
        JobEvent::Rejected { error, .. } => n.error(error),
        JobEvent::BatchDone {
            batch_id,
            succeeded,
            failed,
            ..
        } => n.batch_done(*batch_id, *succeeded, *failed),
        JobEvent::Queued { .. } | JobEvent::Canceled { .. } => {}
    }
}

/// 로그만 남기는 구현 (토스트 TODO)
#[derive(Debug, Default)]
pub struct LogNotifier;

impl Notifier for LogNotifier {
    fn job_started(&self, batch_id: BatchId, job_id: JobId) {
        tracing::info!(batch_id, job_id, "변환 시작");
    }
    fn job_done(&self, job_id: JobId, input: &Path, output: &Path) {
        tracing::info!(job_id, input = %input.display(), output = %output.display(), "변환 완료");
    }
    fn job_failed(&self, job_id: JobId, input: &Path, error: &ErrorInfo) {
        tracing::warn!(job_id, input = %input.display(), code = ?error.code, "변환 실패: {}", error.message);
    }
    fn batch_done(&self, batch_id: BatchId, succeeded: u32, failed: u32) {
        tracing::info!(batch_id, succeeded, failed, "배치 완료");
    }
    fn error(&self, error: &ErrorInfo) {
        tracing::warn!(code = ?error.code, "{}", error.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Record(Mutex<Vec<String>>);
    impl Notifier for Record {
        fn job_started(&self, _: BatchId, j: JobId) {
            self.0.lock().unwrap().push(format!("start {j}"));
        }
        fn job_done(&self, j: JobId, _: &Path, o: &Path) {
            self.0
                .lock()
                .unwrap()
                .push(format!("done {j} {}", o.display()));
        }
        fn job_failed(&self, j: JobId, _: &Path, e: &ErrorInfo) {
            self.0
                .lock()
                .unwrap()
                .push(format!("fail {j} {:?}", e.code));
        }
        fn error(&self, e: &ErrorInfo) {
            self.0.lock().unwrap().push(format!("error {:?}", e.code));
        }
    }

    #[test]
    fn dispatch_routes_events() {
        let r = Record::default();
        let err = convtool_core::CoreError::InvalidInput("x".into()).info();
        dispatch(
            &r,
            &JobEvent::Started {
                batch_id: 1,
                job_id: 2,
                engine: convtool_core::EngineId::Vips,
            },
        );
        dispatch(
            &r,
            &JobEvent::Done {
                batch_id: 1,
                job_id: 2,
                input: "a.png".into(),
                output: "a.webp".into(),
            },
        );
        dispatch(
            &r,
            &JobEvent::Failed {
                batch_id: 1,
                job_id: 3,
                input: "b.png".into(),
                error: err.clone(),
            },
        );
        dispatch(
            &r,
            &JobEvent::Rejected {
                batch_id: 1,
                input: "c.xyz".into(),
                error: err,
            },
        );
        dispatch(
            &LogNotifier,
            &JobEvent::Canceled {
                batch_id: 1,
                job_id: 4,
                input: "d".into(),
            },
        );
        assert_eq!(
            *r.0.lock().unwrap(),
            vec![
                "start 2".to_string(),
                "done 2 a.webp".to_string(),
                "fail 3 InvalidInput".to_string(),
                "error InvalidInput".to_string(),
            ]
        );
    }
}
