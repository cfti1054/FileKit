//! 트레이 아이콘 바쁨 상태 (순수 로직). 대기·실행 중 작업이 하나라도 있으면 바쁨.

use std::collections::HashSet;

use convtool_core::{JobEvent, JobId};

#[derive(Debug, Default)]
pub struct BusyTracker {
    active: HashSet<JobId>,
}

impl BusyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_busy(&self) -> bool {
        !self.active.is_empty()
    }

    /// 이벤트를 반영한다. 바쁨 상태가 바뀌었으면 새 상태를 돌려준다.
    pub fn apply(&mut self, ev: &JobEvent) -> Option<bool> {
        let before = self.is_busy();
        match ev {
            JobEvent::Queued { job_id, .. }
            | JobEvent::Started { job_id, .. }
            | JobEvent::Progress { job_id, .. } => {
                self.active.insert(*job_id);
            }
            JobEvent::Done { job_id, .. }
            | JobEvent::Failed { job_id, .. }
            | JobEvent::Canceled { job_id, .. } => {
                self.active.remove(job_id);
            }
            JobEvent::Rejected { .. } | JobEvent::BatchDone { .. } => {}
        }
        let after = self.is_busy();
        (before != after).then_some(after)
    }
}

/// 바쁨 표시 점 색 (진행 색, RGBA)
pub const BUSY_BADGE_RGBA: [u8; 4] = [0xF7, 0x9A, 0x1E, 0xFF];

/// 트레이 아이콘(RGBA, 행 우선)의 오른쪽 아래에 진행 색 점을 찍은 사본을 만든다.
/// 아이콘 파일을 따로 디코드하지 않으려고 기본 아이콘에서 바쁨 아이콘을 만든다.
/// 크기가 맞지 않는 버퍼는 그대로 복사해 돌려준다.
pub fn badge_rgba(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || out.len() != w * h * 4 {
        return out;
    }
    // 지름 = 짧은 변의 절반, 오른쪽 아래 모서리에 붙인다
    let d = (w.min(h) as f64) / 2.0;
    let r = d / 2.0;
    let cx = w as f64 - r;
    let cy = h as f64 - r;
    let ring = (r * 0.2).max(1.0); // 바깥 흰 테두리 (어두운 작업 표시줄에서도 보이게)
    for y in 0..h {
        for x in 0..w {
            let dx = x as f64 + 0.5 - cx;
            let dy = y as f64 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let i = (y * w + x) * 4;
            if dist <= r - ring {
                out[i..i + 4].copy_from_slice(&BUSY_BADGE_RGBA);
            } else if dist <= r {
                out[i..i + 4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use convtool_core::EngineId;

    #[test]
    fn badge_paints_bottom_right_only() {
        let (w, h) = (32u32, 32u32);
        let src = vec![0u8; (w * h * 4) as usize];
        let out = badge_rgba(&src, w, h);
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        assert_eq!(px(0, 0), [0, 0, 0, 0]);
        assert_eq!(px(31, 0), [0, 0, 0, 0]);
        assert_eq!(px(0, 31), [0, 0, 0, 0]);
        assert_eq!(px(24, 24), BUSY_BADGE_RGBA);
        assert_eq!(out.len(), src.len());
    }

    #[test]
    fn badge_ignores_bad_buffers() {
        assert_eq!(badge_rgba(&[1, 2, 3], 1, 1), vec![1, 2, 3]);
        assert_eq!(badge_rgba(&[], 0, 0), Vec::<u8>::new());
    }
    use std::path::PathBuf;

    fn queued(id: JobId) -> JobEvent {
        JobEvent::Queued {
            batch_id: 1,
            job_id: id,
            input: PathBuf::from("a.mov"),
            to: "mp4".into(),
        }
    }

    #[test]
    fn busy_while_any_job_active() {
        let mut t = BusyTracker::new();
        assert_eq!(t.apply(&queued(1)), Some(true));
        assert_eq!(t.apply(&queued(2)), None);
        assert_eq!(
            t.apply(&JobEvent::Started {
                batch_id: 1,
                job_id: 1,
                engine: EngineId::Ffmpeg
            }),
            None
        );
        assert_eq!(
            t.apply(&JobEvent::Done {
                batch_id: 1,
                job_id: 1,
                input: "a.mov".into(),
                output: "a.mp4".into()
            }),
            None
        );
        assert!(t.is_busy());
        assert_eq!(
            t.apply(&JobEvent::Canceled {
                batch_id: 1,
                job_id: 2,
                input: "b.mov".into()
            }),
            Some(false)
        );
        assert_eq!(
            t.apply(&JobEvent::BatchDone {
                batch_id: 1,
                succeeded: 1,
                failed: 0,
                canceled: 1,
                rejected: 0,
                exit_code: 1
            }),
            None
        );
        assert!(!t.is_busy());
    }

    #[test]
    fn failed_clears_and_unknown_finish_is_harmless() {
        let mut t = BusyTracker::new();
        assert_eq!(
            t.apply(&JobEvent::Failed {
                batch_id: 1,
                job_id: 9,
                input: "x".into(),
                error: convtool_core::CoreError::Canceled.info(),
            }),
            None
        );
        assert_eq!(t.apply(&queued(3)), Some(true));
        assert_eq!(
            t.apply(&JobEvent::Failed {
                batch_id: 1,
                job_id: 3,
                input: "x".into(),
                error: convtool_core::CoreError::Canceled.info(),
            }),
            Some(false)
        );
    }
}
