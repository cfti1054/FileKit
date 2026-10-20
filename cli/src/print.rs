//! 출력 도우미: JSON 줄, 사람용 진행 표시, 표.
//!
//! JSON과 결과 경로는 표준 출력, 사람이 읽을 메시지는 표준 오류로 보낸다.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use convtool_core::{EngineInfo, JobEvent, JobId, PresetInfo};

/// 표준 출력에 한 줄 쓰고 바로 비운다. 파이프가 닫혀도 패닉하지 않는다.
pub fn stdout_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// 표준 오류에 한 줄 쓴다.
pub fn stderr_line(line: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{line}");
}

/// 사람용 출력 한 번 분량
#[derive(Debug, Default, PartialEq, Eq)]
pub struct HumanOutput {
    pub stderr: Option<String>,
    pub stdout: Option<String>,
}

/// `convert`를 `--json` 없이 실행할 때의 진행 표시.
/// 표준 오류: `[1/3] 발표 녹화.mov → MP4 42%` (10% 단위), 표준 출력: 성공한 결과 경로.
pub struct HumanProgress {
    total: usize,
    label: String,
    next_index: usize,
    jobs: HashMap<JobId, (usize, String)>,
    last_step: HashMap<JobId, u32>,
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

impl HumanProgress {
    pub fn new(total: usize, to: &str) -> Self {
        HumanProgress {
            total,
            label: to.to_uppercase(),
            next_index: 0,
            jobs: HashMap::new(),
            last_step: HashMap::new(),
        }
    }

    fn prefix(&self, job_id: JobId) -> String {
        match self.jobs.get(&job_id) {
            Some((i, name)) => format!("[{i}/{}] {name}", self.total),
            None => format!("[?/{}]", self.total),
        }
    }

    pub fn handle(&mut self, ev: &JobEvent) -> HumanOutput {
        let mut out = HumanOutput::default();
        match ev {
            JobEvent::Queued { job_id, input, .. } => {
                self.next_index += 1;
                self.jobs
                    .insert(*job_id, (self.next_index, file_name(input)));
            }
            JobEvent::Rejected { input, error, .. } => {
                self.next_index += 1;
                out.stderr = Some(format!(
                    "[{}/{}] {} 건너뜀: {}",
                    self.next_index,
                    self.total,
                    file_name(input),
                    error.message
                ));
            }
            JobEvent::Started { job_id, engine, .. } => {
                out.stderr = Some(format!(
                    "{} → {} 시작 ({engine})",
                    self.prefix(*job_id),
                    self.label
                ));
            }
            JobEvent::Progress {
                job_id,
                progress: Some(p),
                ..
            } => {
                // 10% 단위로만 쓴다
                let step = (p.clamp(0.0, 1.0) * 10.0).floor() as u32;
                let last = self.last_step.get(job_id).copied();
                if step > 0 && step < 10 && last != Some(step) {
                    self.last_step.insert(*job_id, step);
                    out.stderr = Some(format!(
                        "{} → {} {}%",
                        self.prefix(*job_id),
                        self.label,
                        step * 10
                    ));
                }
            }
            JobEvent::Progress { progress: None, .. } => {}
            JobEvent::Done { job_id, output, .. } => {
                out.stderr = Some(format!(
                    "{} → {} 완료: {}",
                    self.prefix(*job_id),
                    self.label,
                    output.display()
                ));
                out.stdout = Some(output.display().to_string());
            }
            JobEvent::Failed { job_id, error, .. } => {
                out.stderr = Some(format!("{} 실패: {}", self.prefix(*job_id), error.message));
            }
            JobEvent::Canceled { job_id, .. } => {
                out.stderr = Some(format!("{} 취소됨", self.prefix(*job_id)));
            }
            JobEvent::BatchDone {
                succeeded,
                failed,
                canceled,
                rejected,
                ..
            } => {
                out.stderr = Some(format!(
                    "끝: 성공 {succeeded}, 실패 {failed}, 취소 {canceled}, 건너뜀 {rejected}"
                ));
            }
        }
        out
    }
}

/// 사람용 엔진 표
pub fn engines_table(list: &[EngineInfo]) -> String {
    let mut s = String::new();
    for e in list {
        let state = if e.found { "있음" } else { "없음" };
        let required = if e.required { "필수" } else { "선택" };
        let source = e
            .source
            .map(|src| {
                serde_json::to_value(src)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| "-".into());
        let version = e.version.clone().unwrap_or_else(|| "-".into());
        let path = e
            .path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".into());
        s.push_str(&format!(
            "{:<12} {:<4} {:<4} {:<8} {:<16} {}\n",
            e.name, state, required, source, version, path
        ));
        for n in &e.notes {
            s.push_str(&format!("    - {n}\n"));
        }
    }
    s
}

/// 사람용 프리셋 표
pub fn presets_table(list: &[PresetInfo]) -> String {
    let mut s = String::new();
    for p in list {
        s.push_str(&format!(
            "{:<9} {:<8} {}\n",
            p.id.as_str(),
            p.label,
            p.description
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use convtool_core::{EngineId, ErrorCode, ErrorInfo};
    use std::path::PathBuf;

    #[test]
    fn human_progress_lines() {
        let mut h = HumanProgress::new(2, "mp4");
        let input = PathBuf::from(r"C:\영상\발표 녹화.mov");
        assert_eq!(
            h.handle(&JobEvent::Queued {
                batch_id: 1,
                job_id: 7,
                input: input.clone(),
                to: "mp4".into()
            }),
            HumanOutput::default()
        );
        let r = h.handle(&JobEvent::Rejected {
            batch_id: 1,
            input: PathBuf::from("메모.xyz"),
            error: ErrorInfo {
                code: ErrorCode::UnsupportedFormat,
                message: "지원하지 않는 형식입니다: xyz → mp4".into(),
                engine: None,
            },
        });
        assert_eq!(
            r.stderr.as_deref(),
            Some("[2/2] 메모.xyz 건너뜀: 지원하지 않는 형식입니다: xyz → mp4")
        );
        let p = h.handle(&JobEvent::Progress {
            batch_id: 1,
            job_id: 7,
            progress: Some(0.42),
        });
        assert_eq!(p.stderr.as_deref(), Some("[1/2] 발표 녹화.mov → MP4 40%"));
        let p = h.handle(&JobEvent::Progress {
            batch_id: 1,
            job_id: 7,
            progress: Some(0.45),
        });
        assert_eq!(p.stderr, None, "같은 10% 구간");
        let s = h.handle(&JobEvent::Started {
            batch_id: 1,
            job_id: 7,
            engine: EngineId::Ffmpeg,
        });
        assert_eq!(
            s.stderr.as_deref(),
            Some("[1/2] 발표 녹화.mov → MP4 시작 (FFmpeg)")
        );
        let d = h.handle(&JobEvent::Done {
            batch_id: 1,
            job_id: 7,
            input,
            output: PathBuf::from(r"C:\영상\발표 녹화.mp4"),
        });
        assert_eq!(d.stdout.as_deref(), Some(r"C:\영상\발표 녹화.mp4"));
    }

    #[test]
    fn tables_render() {
        let t = presets_table(&convtool_core::presets::list());
        assert!(t.starts_with("web"));
        assert_eq!(t.lines().count(), 3);
        let set = convtool_core::EngineSet::default();
        let t = engines_table(&set.status());
        assert!(t.contains("FFmpeg"));
        assert!(t.contains("없음"));
    }
}
