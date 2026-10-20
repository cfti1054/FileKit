//! 엔진 공통 타입, 단계 계획(plan), 진행률 분배, 취소 토큰.
//!
//! 엔진은 모두 자식 프로세스로 실행한다. 라이브러리로 링크하지 않는다.

pub mod ffmpeg;
pub mod office;
pub mod pandoc;
pub mod vips;

mod locate;
mod runner;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::presets::PresetId;

pub use locate::{EngineConfig, EngineInfo, EngineSet, EngineSource};
pub use runner::run_step;
pub(crate) use runner::{capture_stdout, run_step_with};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineId {
    Ffmpeg,
    Vips,
    Office,
    Pandoc,
}

impl EngineId {
    pub const ALL: [EngineId; 4] = [
        EngineId::Ffmpeg,
        EngineId::Vips,
        EngineId::Office,
        EngineId::Pandoc,
    ];

    /// "FFmpeg", "libvips", "LibreOffice", "Pandoc"
    pub fn display_name(self) -> &'static str {
        match self {
            EngineId::Ffmpeg => "FFmpeg",
            EngineId::Vips => "libvips",
            EngineId::Office => "LibreOffice",
            EngineId::Pandoc => "Pandoc",
        }
    }

    /// Ffmpeg, Vips = true (spec 6장 필수)
    pub fn required(self) -> bool {
        matches!(self, EngineId::Ffmpeg | EngineId::Vips)
    }
}

impl std::fmt::Display for EngineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

/// 진행률을 읽을 스트림
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStream {
    Stdout,
    Stderr,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandSpec {
    pub program: PathBuf,
    /// 셸 문자열 조립 금지
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub progress: ProgressStream,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub engine: EngineId,
    pub command: CommandSpec,
    /// 이 단계가 끝나면 생기는 파일
    pub output: PathBuf,
    /// 전체 진행률 중 비중 (합 1.0)
    pub weight: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct PlanInput<'a> {
    pub input: &'a Path,
    pub input_format: &'a str,
    pub to: &'a str,
    pub preset: PresetId,
    /// 최종 단계가 써야 할 임시 파일 (naming::temp_output)
    pub tmp_output: &'a Path,
    /// 작업별 임시 폴더 (office outdir, 프로필, 중간 docx)
    pub work_dir: &'a Path,
    pub duration_secs: Option<f64>,
}

/// probe 전(`ffmpeg_caps() == None`)에 쓰는 낙관적 인코더 가정.
/// 계획의 "probe 전이면 가능으로 본다"에 맞춘다. 실제로 없으면 엔진 실행이 실패(engine_failed)한다.
pub(crate) fn assumed_ffmpeg_caps() -> ffmpeg::FfmpegCaps {
    ffmpeg::FfmpegCaps {
        h264: Some(ffmpeg::H264Encoder::MediaFoundation),
        vp9: true,
        opus: true,
        mp3lame: true,
    }
}

/// LibreOffice 작업별 프로필 폴더 (work_dir 아래)
pub(crate) fn office_profile_dir(work_dir: &Path) -> PathBuf {
    work_dir.join("lo-profile")
}

fn require(engines: &EngineSet, id: EngineId) -> Result<&Path, CoreError> {
    engines.path(id).ok_or(CoreError::EngineMissing(id))
}

/// 엔진별 모듈을 불러 단계 목록을 만든다. md/html→pdf = pandoc(docx, work_dir) → office(pdf).
/// office는 work_dir에 결과를 쓰므로 마지막에 tmp_output으로 옮기는 일은 job.rs가 Step.output을 보고 한다.
///
/// - office의 `--outdir`은 work_dir, 프로필은 `work_dir/lo-profile`이다.
/// - md/html→pdf의 중간 파일은 `work_dir/<입력 stem>.docx`이고 비중은 pandoc 0.4, office 0.6이다.
/// - ffmpeg caps를 probe하지 않았으면 낙관적 가정([`assumed_ffmpeg_caps`])으로 명령을 만든다.
pub fn plan(engines: &EngineSet, p: &PlanInput) -> Result<Vec<Step>, CoreError> {
    let unsupported = || CoreError::UnsupportedFormat {
        input: p.input_format.to_string(),
        to: Some(p.to.to_string()),
    };
    let entry = crate::formats::output_entry(p.input_format, p.to).ok_or_else(unsupported)?;
    let input_format = crate::formats::normalize_ext(p.input_format).ok_or_else(unsupported)?;
    let to = entry.id;
    for &id in entry.engines {
        require(engines, id)?;
    }
    let single = |engine: EngineId, command: CommandSpec, output: &Path| Step {
        engine,
        command,
        output: output.to_path_buf(),
        weight: 1.0,
    };

    match entry.engines {
        [EngineId::Ffmpeg] => {
            let ff = require(engines, EngineId::Ffmpeg)?;
            let caps = engines
                .ffmpeg_caps()
                .cloned()
                .unwrap_or_else(assumed_ffmpeg_caps);
            let cmd = ffmpeg::build_command(ff, p.input, p.tmp_output, to, p.preset, &caps)?;
            Ok(vec![single(EngineId::Ffmpeg, cmd, p.tmp_output)])
        }
        [EngineId::Vips] => {
            let v = require(engines, EngineId::Vips)?;
            let cmd = vips::build_command(v, p.input, p.tmp_output, to, p.preset)?;
            Ok(vec![single(EngineId::Vips, cmd, p.tmp_output)])
        }
        [EngineId::Pandoc] => {
            let pd = require(engines, EngineId::Pandoc)?;
            let cmd = pandoc::build_command(pd, p.input, input_format, p.tmp_output, to)?;
            Ok(vec![single(EngineId::Pandoc, cmd, p.tmp_output)])
        }
        [EngineId::Office] => {
            let so = require(engines, EngineId::Office)?;
            let profile = office_profile_dir(p.work_dir);
            let cmd = office::build_command(so, p.input, input_format, to, p.work_dir, &profile)?;
            let out = office::expected_output(p.input, input_format, to, p.work_dir);
            Ok(vec![single(EngineId::Office, cmd, &out)])
        }
        [EngineId::Pandoc, EngineId::Office] => {
            let pd = require(engines, EngineId::Pandoc)?;
            let so = require(engines, EngineId::Office)?;
            let stem = p.input.file_stem().unwrap_or_default();
            let mut mid_name = stem.to_os_string();
            mid_name.push(".docx");
            let mid = p.work_dir.join(mid_name);
            let first = pandoc::build_command(pd, p.input, input_format, &mid, "docx")?;
            let profile = office_profile_dir(p.work_dir);
            let second = office::build_command(so, &mid, "docx", to, p.work_dir, &profile)?;
            let out = office::expected_output(&mid, "docx", to, p.work_dir);
            Ok(vec![
                Step {
                    engine: EngineId::Pandoc,
                    command: first,
                    output: mid,
                    weight: 0.4,
                },
                Step {
                    engine: EngineId::Office,
                    command: second,
                    output: out,
                    weight: 0.6,
                },
            ])
        }
        other => Err(CoreError::Internal(format!(
            "형식 표의 엔진 조합을 처리하지 못합니다: {other:?}"
        ))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProgressUpdate {
    Fraction(f64),
    Unknown,
    End,
}

pub fn parse_progress(
    engine: EngineId,
    line: &str,
    duration_secs: Option<f64>,
) -> Option<ProgressUpdate> {
    match engine {
        EngineId::Ffmpeg => ffmpeg::parse_progress(line, duration_secs),
        EngineId::Vips | EngineId::Office | EngineId::Pandoc => None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_id_serde_and_names() {
        let v = serde_json::to_value(EngineId::ALL).expect("직렬화");
        assert_eq!(v, serde_json::json!(["ffmpeg", "vips", "office", "pandoc"]));
        assert_eq!(EngineId::Vips.to_string(), "libvips");
        assert!(EngineId::Ffmpeg.required());
        assert!(EngineId::Vips.required());
        assert!(!EngineId::Office.required());
        assert!(!EngineId::Pandoc.required());
    }

    fn engines_with(ids: &[EngineId]) -> EngineSet {
        let paths = ids
            .iter()
            .map(|&id| (id, PathBuf::from(format!(r"C:\엔진\{id}.exe"))))
            .collect();
        EngineSet::from_parts(paths, None, None)
    }

    fn input<'a>(
        input: &'a Path,
        input_format: &'a str,
        to: &'a str,
        tmp: &'a Path,
        work: &'a Path,
    ) -> PlanInput<'a> {
        PlanInput {
            input,
            input_format,
            to,
            preset: PresetId::Web,
            tmp_output: tmp,
            work_dir: work,
            duration_secs: None,
        }
    }

    #[test]
    fn plan_single_engine_steps() {
        let work = Path::new(r"C:\임시\convtool\1-1");
        let tmp = Path::new(r"C:\영상\.발표 녹화.1.convtool-tmp.mp4");
        let src = Path::new(r"C:\영상\발표 녹화.mov");
        // probe 전(caps None)이어도 mp4 계획은 만들어진다(낙관적 가정)
        let set = engines_with(&EngineId::ALL);
        let steps = plan(&set, &input(src, "mov", "mp4", tmp, work)).expect("ffmpeg 계획");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].engine, EngineId::Ffmpeg);
        assert_eq!(steps[0].output, tmp);
        assert_eq!(steps[0].weight, 1.0);
        assert!(steps[0]
            .command
            .args
            .iter()
            .any(|a| a.as_os_str() == src.as_os_str()));

        let tmp = Path::new(r"C:\사진\.a.1.convtool-tmp.webp");
        let steps = plan(
            &set,
            &input(Path::new(r"C:\사진\a.png"), "png", "webp", tmp, work),
        )
        .expect("vips 계획");
        assert_eq!(steps[0].engine, EngineId::Vips);
        assert_eq!(steps[0].output, tmp);

        let tmp = Path::new(r"C:\문서\.보고서.1.convtool-tmp.pdf");
        let doc = Path::new(r"C:\문서\보고서.docx");
        let steps = plan(&set, &input(doc, "docx", "pdf", tmp, work)).expect("office 계획");
        assert_eq!(steps[0].engine, EngineId::Office);
        assert_eq!(
            steps[0].output,
            office::expected_output(doc, "docx", "pdf", work)
        );
        assert!(steps[0].output.starts_with(work));
    }

    #[test]
    fn plan_markdown_pdf_two_steps() {
        let work = Path::new(r"C:\임시\convtool\1-2");
        let tmp = Path::new(r"C:\문서\.메모 정리.2.convtool-tmp.pdf");
        let src = Path::new(r"C:\문서\메모 정리.md");
        let set = engines_with(&[EngineId::Pandoc, EngineId::Office]);
        let steps = plan(&set, &input(src, "md", "pdf", tmp, work)).expect("계획");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].engine, EngineId::Pandoc);
        assert_eq!(steps[0].output, work.join("메모 정리.docx"));
        assert_eq!(steps[1].engine, EngineId::Office);
        assert_eq!(
            steps[1].output,
            office::expected_output(&work.join("메모 정리.docx"), "docx", "pdf", work)
        );
        let total: f64 = steps.iter().map(|s| s.weight).sum();
        assert!((total - 1.0).abs() < 1e-9);
    }

    #[test]
    fn plan_errors() {
        let work = Path::new("w");
        let tmp = Path::new("t.pdf");
        let set = engines_with(&[EngineId::Pandoc]);
        let err =
            plan(&set, &input(Path::new("a.md"), "md", "pdf", tmp, work)).expect_err("office 없음");
        assert!(matches!(err, CoreError::EngineMissing(EngineId::Office)));
        let err =
            plan(&set, &input(Path::new("a.mp4"), "mp4", "mp4", tmp, work)).expect_err("항등");
        assert_eq!(err.code(), crate::error::ErrorCode::UnsupportedFormat);
        let err =
            plan(&set, &input(Path::new("a.xyz"), "xyz", "pdf", tmp, work)).expect_err("미지원");
        assert_eq!(err.code(), crate::error::ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn cancel_token_shared() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!b.is_canceled());
        a.cancel();
        assert!(b.is_canceled());
    }
}
