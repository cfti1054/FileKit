// 릴리스 빌드에서는 콘솔 창을 띄우지 않는다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CONVTOOL_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let mode = match convtool_app::args::parse_from(std::env::args_os()) {
        Ok(m) => m,
        Err(e) => {
            // --help/--version도 여기로 온다. 릴리스 빌드(GUI 서브시스템)에서는 보이지 않는다.
            let _ = e.print();
            return if e.use_stderr() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            };
        }
    };

    run(mode)
}

#[cfg(feature = "tauri-app")]
fn run(mode: convtool_app::args::AppMode) -> ExitCode {
    convtool_app::app::run(mode)
}

#[cfg(not(feature = "tauri-app"))]
fn run(mode: convtool_app::args::AppMode) -> ExitCode {
    tracing::error!(
        ?mode,
        "tauri-app feature 없이 빌드되어 앱을 실행할 수 없습니다 (순수 테스트용 빌드)"
    );
    ExitCode::from(1)
}
