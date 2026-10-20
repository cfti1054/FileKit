//! 단계 실행기.
//!
//! - 창을 띄우지 않는다(`CREATE_NO_WINDOW`).
//! - 진행 스트림을 줄 단위로 읽어 진행률을 알린다.
//! - 취소하면 자식 프로세스를 죽인다. Windows에서는 Job Object에 넣어 손자 프로세스
//!   (예: soffice.exe가 띄운 soffice.bin)까지 함께 끝낸다.
//! - 실패하면 표준 오류 마지막 20줄을 원인으로 남긴다.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    parse_progress, CancelToken, CommandSpec, EngineId, ProgressStream, ProgressUpdate, Step,
};
use crate::error::CoreError;

/// 실패 원인으로 남길 표준 오류 줄 수
pub(crate) const STDERR_TAIL_LINES: usize = 20;
/// 취소·종료 확인 간격
const POLL: Duration = Duration::from_millis(50);
/// 프로세스가 끝난 뒤 남은 출력 줄을 기다리는 최대 시간 (손자 프로세스가 파이프를 쥐고 있을 때 대비)
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 창 없이 실행할 `Command`를 만든다. 셸을 거치지 않는다.
pub(crate) fn base_command(spec: &CommandSpec) -> Command {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn spawn_error(engine: EngineId, program: &Path, e: std::io::Error) -> CoreError {
    if e.kind() == std::io::ErrorKind::NotFound {
        CoreError::EngineMissing(engine)
    } else {
        CoreError::Io {
            path: program.to_path_buf(),
            source: e,
        }
    }
}

/// runner.rs 구현. CREATE_NO_WINDOW, 진행 스트림 줄 읽기, 취소 시 child.kill(), 실패 시 stderr 마지막 20줄.
///
/// `Step`에는 길이 정보가 없으므로 FFmpeg 진행률은 비율을 모른다(`None`).
/// 작업 큐는 길이를 함께 넘기는 [`run_step_with`]를 쓴다.
pub fn run_step(
    step: &Step,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Option<f64>),
) -> Result<(), CoreError> {
    run_step_with(step, None, cancel, on_progress)
}

enum Msg {
    Line(String),
    Eof,
}

/// 줄을 읽어 보낸다. 엔진 출력이 UTF-8이 아닐 수 있어 손실 변환한다.
fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    forward: bool,
    tail: Option<Arc<Mutex<VecDeque<String>>>>,
    tx: mpsc::Sender<Msg>,
) {
    std::thread::spawn(move || {
        let mut r = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\r', '\n'])
                        .to_string();
                    if let Some(tail) = &tail {
                        if let Ok(mut t) = tail.lock() {
                            if !line.trim().is_empty() {
                                if t.len() == STDERR_TAIL_LINES {
                                    t.pop_front();
                                }
                                t.push_back(line.clone());
                            }
                        }
                    }
                    if forward && tx.send(Msg::Line(line)).is_err() {
                        // 받는 쪽이 끝났다. 파이프가 막히지 않게 계속 읽어 버린다.
                    }
                }
            }
        }
        let _ = tx.send(Msg::Eof);
    });
}

/// `run_step`에 미디어 길이(초)를 더한 버전. 작업 큐가 쓴다.
pub(crate) fn run_step_with(
    step: &Step,
    duration_secs: Option<f64>,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Option<f64>),
) -> Result<(), CoreError> {
    if cancel.is_canceled() {
        return Err(CoreError::Canceled);
    }
    let spec = &step.command;
    let mut cmd = base_command(spec);
    cmd.stdin(Stdio::null()).stderr(Stdio::piped());
    if spec.progress == ProgressStream::Stdout {
        cmd.stdout(Stdio::piped());
    } else {
        cmd.stdout(Stdio::null());
    }
    tracing::debug!(engine = %step.engine, program = %spec.program.display(), args = ?spec.args, "엔진 실행");
    let mut child = cmd
        .spawn()
        .map_err(|e| spawn_error(step.engine, &spec.program, e))?;
    let tree = ProcessTree::attach(&child);

    let tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
    let (tx, rx) = mpsc::channel::<Msg>();
    let mut readers = 0usize;
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(
            stderr,
            spec.progress == ProgressStream::Stderr,
            Some(Arc::clone(&tail)),
            tx.clone(),
        );
        readers += 1;
    }
    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, true, None, tx.clone());
        readers += 1;
    }
    drop(tx);

    on_progress(None);
    let mut handle_line = |line: &str| match parse_progress(step.engine, line, duration_secs) {
        Some(ProgressUpdate::Fraction(f)) => on_progress(Some(f.clamp(0.0, 1.0))),
        Some(ProgressUpdate::Unknown) => on_progress(None),
        Some(ProgressUpdate::End) => on_progress(Some(1.0)),
        None => {}
    };

    let mut eofs = 0usize;
    let status: ExitStatus = loop {
        if cancel.is_canceled() {
            kill(&mut child, &tree);
            return Err(CoreError::Canceled);
        }
        match rx.recv_timeout(POLL) {
            Ok(Msg::Line(l)) => handle_line(&l),
            Ok(Msg::Eof) => eofs += 1,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(POLL),
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => {
                kill(&mut child, &tree);
                return Err(CoreError::Io {
                    path: spec.program.clone(),
                    source: e,
                });
            }
        }
    };

    // 남은 줄 읽기 (상한 있음)
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    while eofs < readers {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            tracing::debug!(engine = %step.engine, "출력 파이프가 닫히지 않음: 남은 줄 무시");
            break;
        }
        match rx.recv_timeout(left) {
            Ok(Msg::Line(l)) => handle_line(&l),
            Ok(Msg::Eof) => eofs += 1,
            Err(_) => break,
        }
    }
    // 남은 손자 프로세스 정리
    drop(tree);

    if status.success() {
        return Ok(());
    }
    if cancel.is_canceled() {
        return Err(CoreError::Canceled);
    }
    let stderr_tail = tail
        .lock()
        .map(|t| t.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    Err(CoreError::EngineFailed {
        engine: step.engine,
        code: status.code(),
        stderr_tail,
    })
}

fn kill(child: &mut Child, tree: &ProcessTree) {
    tree.terminate();
    let _ = child.kill();
    let _ = child.wait();
}

/// 프로세스를 실행해 표준 출력을 모은다(probe용). 제한 시간을 넘기면 죽이고 Err.
/// 종료 코드가 0이 아니어도 표준 출력이 있으면 돌려준다(일부 엔진은 버전 출력 후 1로 끝난다).
pub(crate) fn capture_stdout(spec: &CommandSpec, timeout: Duration) -> Result<String, String> {
    let mut cmd = base_command(spec);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("실행 실패 ({}): {e}", spec.program.display()))?;
    let tree = ProcessTree::attach(&child);
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    if let Some(mut out) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    } else {
        drop(tx);
    }
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => {
                kill(&mut child, &tree);
                return Err(format!("대기 실패: {e}"));
            }
        }
        if Instant::now() >= deadline {
            kill(&mut child, &tree);
            return Err(format!("{}초 안에 끝나지 않음", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let left = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(500));
    let out = rx.recv_timeout(left).unwrap_or_default();
    drop(tree);
    let text = String::from_utf8_lossy(&out).into_owned();
    if status.success() || !text.trim().is_empty() {
        Ok(text)
    } else {
        Err(format!("종료 코드 {:?}", status.code()))
    }
}

// ===== 프로세스 트리 정리 =====

#[cfg(windows)]
mod tree {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// 자식을 넣은 Job Object. 닫히면(KILL_ON_JOB_CLOSE) 안에 남은 프로세스가 모두 끝난다.
    ///
    /// 한계: spawn과 할당 사이(수 μs)에 자식이 띄운 손자는 Job에 들어가지 않는다.
    pub(super) struct ProcessTree(HANDLE);

    // HANDLE은 원시 포인터지만 커널 객체 핸들이라 스레드 사이에 옮겨도 된다.
    unsafe impl Send for ProcessTree {}

    impl ProcessTree {
        pub(super) fn attach(child: &Child) -> Self {
            // SAFETY: 모든 포인터 인자는 null이거나 지역 변수의 유효한 주소다.
            // 반환 핸들은 이 구조체가 소유하고 Drop에서 한 번만 닫는다.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    tracing::debug!("Job Object 생성 실패: 프로세스 트리 정리 없이 진행");
                    return ProcessTree(std::ptr::null_mut());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(info).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
                    tracing::debug!("Job Object 설정 실패: 프로세스 트리 정리 없이 진행");
                    CloseHandle(job);
                    return ProcessTree(std::ptr::null_mut());
                }
                ProcessTree(job)
            }
        }

        pub(super) fn terminate(&self) {
            if !self.0.is_null() {
                // SAFETY: self.0은 attach에서 만든 유효한 Job 핸들이다.
                unsafe {
                    TerminateJobObject(self.0, 1);
                }
            }
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: 소유한 핸들을 한 번만 닫는다. KILL_ON_JOB_CLOSE로 남은 프로세스가 끝난다.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod tree {
    use std::process::Child;

    /// Windows가 아닌 곳에서는 자식만 죽인다.
    pub(super) struct ProcessTree;

    impl ProcessTree {
        pub(super) fn attach(_child: &Child) -> Self {
            ProcessTree
        }

        pub(super) fn terminate(&self) {}
    }
}

use tree::ProcessTree;

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn cmd_exe() -> PathBuf {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        PathBuf::from(root).join("System32").join("cmd.exe")
    }

    fn step(args: &[&str], progress: ProgressStream) -> Step {
        Step {
            engine: EngineId::Vips,
            command: CommandSpec {
                program: cmd_exe(),
                args: args.iter().map(OsString::from).collect(),
                cwd: None,
                progress,
            },
            output: PathBuf::from("unused"),
            weight: 1.0,
        }
    }

    #[test]
    fn success_reports_initial_unknown_progress() {
        let mut seen = Vec::new();
        run_step(
            &step(&["/C", "echo hello"], ProgressStream::Stdout),
            &CancelToken::new(),
            &mut |p| seen.push(p),
        )
        .expect("성공");
        assert_eq!(seen.first(), Some(&None));
    }

    #[test]
    fn failure_keeps_last_20_stderr_lines() {
        let mut script = String::new();
        for i in 1..=25 {
            script.push_str(&format!("echo line{i} 1>&2& "));
        }
        script.push_str("exit 3");
        let err = run_step(
            &step(&["/C", &script], ProgressStream::None),
            &CancelToken::new(),
            &mut |_| {},
        )
        .expect_err("실패");
        match err {
            CoreError::EngineFailed {
                engine,
                code,
                stderr_tail,
            } => {
                assert_eq!(engine, EngineId::Vips);
                assert_eq!(code, Some(3));
                let lines: Vec<&str> = stderr_tail.lines().map(str::trim).collect();
                assert_eq!(lines.len(), 20, "{stderr_tail}");
                assert_eq!(lines[0], "line6");
                assert_eq!(lines[19], "line25");
            }
            other => panic!("예상 밖 오류: {other:?}"),
        }
    }

    #[test]
    fn cancel_kills_running_process() {
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            c2.cancel();
        });
        let start = Instant::now();
        let err = run_step(
            &step(&["/C", "ping -n 30 127.0.0.1 >NUL"], ProgressStream::None),
            &cancel,
            &mut |_| {},
        )
        .expect_err("취소");
        t.join().expect("취소 스레드");
        assert!(matches!(err, CoreError::Canceled), "{err:?}");
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn already_canceled_does_not_spawn() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = run_step(
            &step(&["/C", "echo x"], ProgressStream::None),
            &cancel,
            &mut |_| {},
        )
        .expect_err("취소");
        assert!(matches!(err, CoreError::Canceled));
    }

    #[test]
    fn missing_program_is_engine_missing() {
        let mut s = step(&[], ProgressStream::None);
        s.command.program = PathBuf::from(r"C:\없는 폴더\없음.exe");
        let err = run_step(&s, &CancelToken::new(), &mut |_| {}).expect_err("없음");
        assert!(
            matches!(err, CoreError::EngineMissing(EngineId::Vips)),
            "{err:?}"
        );
    }

    #[test]
    fn capture_stdout_reads_output_and_times_out() {
        let spec = CommandSpec {
            program: cmd_exe(),
            args: vec!["/C".into(), "echo 버전 1.2".into()],
            cwd: None,
            progress: ProgressStream::None,
        };
        let out = capture_stdout(&spec, Duration::from_secs(10)).expect("출력");
        assert!(out.contains("1.2"), "{out}");

        let spec = CommandSpec {
            program: cmd_exe(),
            args: vec!["/C".into(), "ping -n 30 127.0.0.1 >NUL".into()],
            cwd: None,
            progress: ProgressStream::None,
        };
        let start = Instant::now();
        assert!(capture_stdout(&spec, Duration::from_millis(300)).is_err());
        assert!(start.elapsed() < Duration::from_secs(10));
    }
}
