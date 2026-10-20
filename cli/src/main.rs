//! `convtool` 명령줄 도구.
//!
//! ```text
//! convtool formats <파일...> [--primary <파일>]
//! convtool convert <파일...> --to <형식> [--preset web|original|small] [--out <폴더>] [--overwrite] [--jobs <N>] [--json]
//! convtool presets [--json]
//! convtool engines [--json]
//! ```
//! 종료 코드: 0 성공, 1 일부 실패, 2 지원하지 않는 형식, 3 엔진 없음. 인자 오류는 clap 기본값.

mod args;
mod print;
mod run;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();

    let cli = args::Cli::parse();
    match run::run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            print::stderr_line(&format!("오류: {e:#}"));
            ExitCode::from(1)
        }
    }
}
