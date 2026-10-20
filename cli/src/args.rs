//! 명령줄 인자 (clap 4 derive). `--json`은 전역 플래그다.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use convtool_core::PresetId;

#[derive(Debug, Parser)]
#[command(name = "convtool", version = convtool_core::VERSION, about = "변환툴 명령줄 도구")]
pub struct Cli {
    /// 표준 출력에 JSON으로 쓴다 (convert: 한 줄에 이벤트 하나)
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 파일들이 바꿀 수 있는 형식과 원형 메뉴 배치를 JSON으로 출력한다 (항상 JSON)
    Formats {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// 기준 파일. 이 파일과 다른 분류의 파일은 뺀다
        #[arg(long)]
        primary: Option<PathBuf>,
    },
    /// 파일을 변환한다
    Convert {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// 출력 형식 (mp4, mp3, gif, webm, wav, flac, jpg, png, webp, pdf, txt, docx, html)
        #[arg(long)]
        to: String,
        /// 프리셋: web | original | small
        #[arg(long, default_value = "web")]
        preset: PresetId,
        /// 출력 폴더 (없으면 원본 폴더, 폴더가 없으면 만든다)
        #[arg(long)]
        out: Option<PathBuf>,
        /// 같은 이름의 파일이 있으면 덮어쓴다 (원본은 덮어쓰지 않는다)
        #[arg(long)]
        overwrite: bool,
        /// 동시에 실행할 작업 수 (기본 2)
        #[arg(long)]
        jobs: Option<usize>,
    },
    /// 프리셋 목록
    Presets,
    /// 엔진 상태
    Engines,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_convert() {
        let cli = Cli::try_parse_from([
            "convtool",
            "convert",
            "발표 녹화.mov",
            "b.mov",
            "--to",
            "mp4",
            "--preset",
            "small",
            "--json",
            "--jobs",
            "3",
        ])
        .expect("해석");
        assert!(cli.json);
        match cli.command {
            Command::Convert {
                files,
                to,
                preset,
                jobs,
                overwrite,
                out,
            } => {
                assert_eq!(files.len(), 2);
                assert_eq!(to, "mp4");
                assert_eq!(preset, PresetId::Small);
                assert_eq!(jobs, Some(3));
                assert!(!overwrite);
                assert_eq!(out, None);
            }
            other => panic!("예상 밖: {other:?}"),
        }
        assert!(Cli::try_parse_from([
            "convtool", "convert", "a.mov", "--to", "mp4", "--preset", "x"
        ])
        .is_err());
        assert!(Cli::try_parse_from(["convtool", "convert", "--to", "mp4"]).is_err());
    }
}
