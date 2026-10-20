//! 명령 실행. 1단계 CLI는 파이프를 거치지 않고 코어를 직접 쓴다.
//!
//! 종료 코드: 0 성공, 1 일부 실패, 2 지원하지 않는 형식, 3 엔진 없음.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use convtool_core::{
    presets, query_formats, EngineConfig, EngineSet, EventSink, JobEvent, JobQueue, JobRequest,
    PresetId, QueueConfig,
};
use serde_json::json;

use crate::args::{Cli, Command};
use crate::print::{self, HumanProgress};

pub fn run(cli: Cli) -> anyhow::Result<u8> {
    match cli.command {
        Command::Formats { files, primary } => Ok(formats(&files, primary.as_deref())),
        Command::Convert {
            files,
            to,
            preset,
            out,
            overwrite,
            jobs,
        } => Ok(convert(
            JobRequest {
                files,
                to,
                preset,
                out_dir: out,
                overwrite,
            },
            jobs,
            cli.json,
        )),
        Command::Presets => presets_cmd(cli.json),
        Command::Engines => engines_cmd(cli.json),
    }
}

fn engines() -> EngineSet {
    let mut set = EngineSet::locate(&EngineConfig::from_env());
    set.probe();
    set
}

/// 항상 JSON 한 덩어리: 성공 = FormatsReport, 실패 = {"error": ErrorInfo}
fn formats(files: &[PathBuf], primary: Option<&std::path::Path>) -> u8 {
    let set = engines();
    match query_formats(files, primary, &set) {
        Ok(report) => match serde_json::to_string(&report) {
            Ok(s) => {
                print::stdout_line(&s);
                0
            }
            Err(e) => {
                print::stderr_line(&format!("오류: {e}"));
                1
            }
        },
        Err(e) => {
            let info = e.info();
            print::stdout_line(&json!({ "error": info }).to_string());
            print::stderr_line(&info.message);
            info.code.cli_exit_code()
        }
    }
}

fn convert(request: JobRequest, jobs: Option<usize>, json_mode: bool) -> u8 {
    let set = Arc::new(engines());
    let config = QueueConfig {
        max_parallel: jobs.unwrap_or(QueueConfig::default().max_parallel).max(1),
        ..QueueConfig::default()
    };
    let sink: EventSink = if json_mode {
        Arc::new(|ev: &JobEvent| match serde_json::to_string(ev) {
            Ok(line) => print::stdout_line(&line),
            Err(e) => tracing::error!("이벤트 직렬화 실패: {e}"),
        })
    } else {
        let human = Mutex::new(HumanProgress::new(request.files.len(), &request.to));
        Arc::new(move |ev: &JobEvent| {
            let out = human.lock().map(|mut h| h.handle(ev)).unwrap_or_default();
            if let Some(s) = out.stderr {
                print::stderr_line(&s);
            }
            if let Some(s) = out.stdout {
                print::stdout_line(&s);
            }
        })
    };

    let queue = Arc::new(JobQueue::new(set, config, sink));
    {
        let q = Arc::clone(&queue);
        if let Err(e) = ctrlc::set_handler(move || {
            print::stderr_line("취소하는 중…");
            q.cancel_all();
        }) {
            tracing::warn!("Ctrl+C 처리기를 설치하지 못했습니다: {e}");
        }
    }

    let code = match queue.submit(request) {
        Ok(ticket) => match queue.wait_batch(ticket.batch_id) {
            Some(summary) => summary.exit_code(),
            None => {
                print::stderr_line("오류: 배치 결과를 찾지 못했습니다");
                1
            }
        },
        // 배치를 만들기 전 실패: --json이면 formats와 같은 {"error": ErrorInfo} 한 줄
        Err(e) => {
            let info = e.info();
            if json_mode {
                print::stdout_line(&json!({ "error": info }).to_string());
            }
            print::stderr_line(&format!("오류: {}", info.message));
            info.code.cli_exit_code()
        }
    };
    queue.shutdown();
    code
}

fn presets_cmd(json_mode: bool) -> anyhow::Result<u8> {
    let list = presets::list();
    if json_mode {
        print::stdout_line(&serde_json::to_string(&list)?);
    } else {
        let default = PresetId::default();
        print::stderr_line(&format!("기본 프리셋: {default}"));
        print::stdout_line(print::presets_table(&list).trim_end());
    }
    Ok(0)
}

fn engines_cmd(json_mode: bool) -> anyhow::Result<u8> {
    let status = engines().status();
    if json_mode {
        print::stdout_line(&serde_json::to_string(&status)?);
    } else {
        print::stdout_line(print::engines_table(&status).trim_end());
    }
    Ok(0)
}
