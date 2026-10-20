//! 두 번째 실행(창 모드, `convert --ask`)이 트레이 서버에 요청 하나를 넘기는 클라이언트.

use std::io;
use std::time::{Duration, Instant};

use convtool_core::pipe::{
    decode_server, encode_line, ClientKind, ClientMessage, ServerMessage, MAX_LINE_BYTES,
    PIPE_NAME, PROTOCOL_VERSION,
};
use convtool_core::ErrorInfo;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PIPE_BUSY: i32 = 231;
const CONNECT_WAIT: Duration = Duration::from_secs(2);
/// 서버는 엔진 확인(probe)이 끝날 때까지 응답을 미룰 수 있다
const RESPONSE_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum SendError {
    /// 트레이가 실행 중이 아니다 (파이프 없음)
    NotRunning,
    Io(io::Error),
    /// 서버가 `error`로 답했다
    Server(ErrorInfo),
    /// 예상하지 못한 응답
    Protocol(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NotRunning => write!(f, "트레이가 실행 중이 아닙니다"),
            SendError::Io(e) => write!(f, "파이프 입출력 오류: {e}"),
            SendError::Server(e) => write!(f, "서버 오류: {}", e.message),
            SendError::Protocol(m) => write!(f, "파이프 프로토콜 오류: {m}"),
        }
    }
}

impl std::error::Error for SendError {}

async fn connect() -> Result<NamedPipeClient, SendError> {
    let deadline = Instant::now() + CONNECT_WAIT;
    loop {
        match ClientOptions::new().open(PIPE_NAME) {
            Ok(c) => return Ok(c),
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => {
                return Err(SendError::NotRunning)
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(SendError::Io(e)),
        }
    }
}

async fn read_msg<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<ServerMessage, SendError> {
    let mut buf = Vec::new();
    let n = tokio::time::timeout(
        RESPONSE_WAIT,
        (&mut *reader)
            .take((MAX_LINE_BYTES + 1) as u64)
            .read_until(b'\n', &mut buf),
    )
    .await
    .map_err(|_| SendError::Protocol("응답 시간 초과".into()))?
    .map_err(SendError::Io)?;
    if n == 0 {
        return Err(SendError::Protocol("서버가 연결을 닫았습니다".into()));
    }
    let line = String::from_utf8(buf).map_err(|_| SendError::Protocol("UTF-8 아님".into()))?;
    decode_server(&line).map_err(|e| SendError::Protocol(e.to_string()))
}

/// 트레이가 오버레이·메인 창을 앞으로 가져올 수 있게 전경 권한을 넘긴다.
/// 셸이 띄운 이 프로세스는 전경 권한이 있지만 트레이 프로세스는 없다.
fn allow_foreground() {
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
    // SAFETY: 인자 하나짜리 단순 API. 실패해도(전경 권한 없음) 동작에는 문제없다.
    if let Err(e) = unsafe { AllowSetForegroundWindow(ASFW_ANY) } {
        tracing::debug!("AllowSetForegroundWindow 실패: {e}");
    }
}

/// hello를 주고받은 뒤 요청 하나를 보내고 첫 응답을 돌려준다.
pub async fn send_one(kind: ClientKind, msg: ClientMessage) -> Result<ServerMessage, SendError> {
    let pipe = connect().await?;
    let (rd, mut wr) = tokio::io::split(pipe);
    let mut reader = BufReader::new(rd);

    let hello = ClientMessage::Hello {
        protocol: PROTOCOL_VERSION,
        client: kind,
        pid: std::process::id(),
    };
    let line = encode_line(&hello).map_err(|e| SendError::Protocol(e.to_string()))?;
    wr.write_all(line.as_bytes()).await.map_err(SendError::Io)?;
    match read_msg(&mut reader).await? {
        ServerMessage::HelloAck { .. } => {}
        ServerMessage::Error { error, .. } => return Err(SendError::Server(error)),
        other => return Err(SendError::Protocol(format!("hello_ack 대신 {other:?}"))),
    }

    allow_foreground();
    let line = encode_line(&msg).map_err(|e| SendError::Protocol(e.to_string()))?;
    wr.write_all(line.as_bytes()).await.map_err(SendError::Io)?;
    wr.flush().await.map_err(SendError::Io)?;
    match read_msg(&mut reader).await? {
        ServerMessage::Error { error, .. } => Err(SendError::Server(error)),
        other => Ok(other),
    }
}
