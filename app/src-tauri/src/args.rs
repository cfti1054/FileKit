//! 앱 실행 인자. 순수 로직(clap)이라 tauri 없이 테스트한다.
//!
//! - `변환툴.exe`                     → 창 모드
//! - `변환툴.exe --tray`              → 트레이 상주 (시작 시 실행 Run 키)
//! - `변환툴.exe convert <파일...> --ask` → 우클릭 메뉴 (트레이가 있으면 파이프 `show_menu`)

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    Window,
    Tray,
    ConvertAsk { files: Vec<PathBuf> },
}

#[derive(Debug, Parser)]
#[command(
    name = "변환툴",
    version,
    about = "변환툴 트레이 앱",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// 트레이에 상주한다 (휠 클릭 원형 메뉴, 파이프 서버)
    #[arg(long)]
    tray: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 파일을 변환한다. 지금은 `--ask`(원형 메뉴로 형식 묻기)만 지원한다.
    Convert {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// 형식을 원형 메뉴로 묻는다
        #[arg(long, required = true)]
        ask: bool,
    },
}

/// 인자를 해석한다. 첫 항목은 실행 파일 경로다.
pub fn parse_from<I, T>(args: I) -> Result<AppMode, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(args)?;
    Ok(match cli.command {
        Some(Command::Convert { files, ask: _ }) => AppMode::ConvertAsk { files },
        None if cli.tray => AppMode::Tray,
        None => AppMode::Window,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_is_window() {
        assert_eq!(parse_from(["변환툴.exe"]).unwrap(), AppMode::Window);
    }

    #[test]
    fn tray_flag() {
        assert_eq!(parse_from(["변환툴.exe", "--tray"]).unwrap(), AppMode::Tray);
    }

    #[test]
    fn convert_ask_keeps_spaces_and_korean() {
        let m = parse_from(["변환툴.exe", "convert", r"C:\영상\발표 녹화.mov", "--ask"]).unwrap();
        assert_eq!(
            m,
            AppMode::ConvertAsk {
                files: vec![PathBuf::from(r"C:\영상\발표 녹화.mov")]
            }
        );
    }

    #[test]
    fn convert_ask_before_files_and_multiple() {
        let m = parse_from(["x.exe", "convert", "--ask", "a.mp4", "b.mov"]).unwrap();
        assert_eq!(
            m,
            AppMode::ConvertAsk {
                files: vec![PathBuf::from("a.mp4"), PathBuf::from("b.mov")]
            }
        );
    }

    #[test]
    fn convert_without_ask_is_error() {
        assert!(parse_from(["x.exe", "convert", "a.mp4"]).is_err());
    }

    #[test]
    fn convert_without_files_is_error() {
        assert!(parse_from(["x.exe", "convert", "--ask"]).is_err());
    }

    #[test]
    fn tray_with_subcommand_is_error() {
        assert!(parse_from(["x.exe", "--tray", "convert", "a.mp4", "--ask"]).is_err());
    }

    #[test]
    fn unknown_flag_is_error() {
        assert!(parse_from(["x.exe", "--bogus"]).is_err());
    }
}
