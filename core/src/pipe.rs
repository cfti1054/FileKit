//! 이름 있는 파이프 메시지 serde 타입과 줄 인코딩. Win32 코드는 넣지 않는다.
//!
//! 전송 규칙: UTF-8 JSON 한 줄에 하나(`\n` 구분), 한 줄 최대 1 MiB.
//! 클라이언트는 연결하자마자 `hello`를 보낸다.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorInfo};
use crate::formats::FormatsReport;
use crate::job::{BatchTicket, JobEvent, JobId, JobRequest};

pub const PIPE_NAME: &str = r"\\.\pipe\convtool";
pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_LINE_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Cli,
    Window,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol: u32,
        client: ClientKind,
        pid: u32,
    },
    QueryFormats {
        request_id: u64,
        files: Vec<PathBuf>,
        primary: Option<PathBuf>,
    },
    Submit {
        request_id: u64,
        request: JobRequest,
    },
    Cancel {
        request_id: u64,
        job_id: JobId,
    },
    Subscribe {
        request_id: u64,
    },
    ShowMenu {
        request_id: u64,
        files: Vec<PathBuf>,
        primary: Option<PathBuf>,
    },
    ShowMainWindow {
        request_id: u64,
        files: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    HelloAck {
        protocol: u32,
        server_version: String,
    },
    Formats {
        request_id: u64,
        report: FormatsReport,
    },
    Submitted {
        request_id: u64,
        ticket: BatchTicket,
    },
    Ack {
        request_id: u64,
        ok: bool,
    },
    Error {
        request_id: Option<u64>,
        error: ErrorInfo,
    },
    Event {
        event: JobEvent,
    },
}

/// 메시지를 JSON 한 줄로 만든다. 끝에 '\n'.
pub fn encode_line<T: Serialize>(msg: &T) -> Result<String, CoreError> {
    let mut line = serde_json::to_string(msg)
        .map_err(|e| CoreError::Internal(format!("파이프 메시지 직렬화 실패: {e}")))?;
    if line.len() + 1 > MAX_LINE_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "파이프 메시지가 너무 깁니다 ({} 바이트, 최대 {MAX_LINE_BYTES})",
            line.len() + 1
        )));
    }
    line.push('\n');
    Ok(line)
}

/// 길이 초과·형식 오류 → InvalidInput
pub fn decode_client(line: &str) -> Result<ClientMessage, CoreError> {
    decode(line)
}

/// 길이 초과·형식 오류 → InvalidInput
pub fn decode_server(line: &str) -> Result<ServerMessage, CoreError> {
    decode(line)
}

fn decode<T: serde::de::DeserializeOwned>(line: &str) -> Result<T, CoreError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(CoreError::InvalidInput(format!(
            "파이프 메시지가 너무 깁니다 ({} 바이트, 최대 {MAX_LINE_BYTES})",
            line.len()
        )));
    }
    let trimmed = line.trim_end_matches(['\n', '\r']);
    serde_json::from_str(trimmed)
        .map_err(|e| CoreError::InvalidInput(format!("파이프 메시지 형식 오류: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::EngineId;
    use crate::error::ErrorCode;
    use crate::formats::{
        Category, FileFormat, FormatOption, RadialLayout, RadialSlot, SkippedFile,
    };
    use crate::job::QueuedJob;
    use crate::presets::PresetId;
    use serde_json::Value;

    /// 설계 계획 (2) 클라이언트 → 서버 예시
    const CLIENT_LINES: [&str; 7] = [
        r#"{"type":"hello","protocol":1,"client":"shell","pid":4120}"#,
        r#"{"type":"query_formats","request_id":1,"files":["C:\\영상\\발표 녹화.mov"],"primary":"C:\\영상\\발표 녹화.mov"}"#,
        r#"{"type":"submit","request_id":2,"request":{"files":["C:\\영상\\발표 녹화.mov"],"to":"mp4","preset":"web","out_dir":null,"overwrite":false}}"#,
        r#"{"type":"cancel","request_id":3,"job_id":7}"#,
        r#"{"type":"subscribe","request_id":4}"#,
        r#"{"type":"show_menu","request_id":5,"files":["C:\\영상\\발표 녹화.mov"],"primary":"C:\\영상\\발표 녹화.mov"}"#,
        r#"{"type":"show_main_window","request_id":6,"files":[]}"#,
    ];

    /// 설계 계획 (2) 서버 → 클라이언트 예시 (formats는 report 자리 표시가 있어 따로 검사)
    const SERVER_LINES: [&str; 5] = [
        r#"{"type":"hello_ack","protocol":1,"server_version":"0.1.0"}"#,
        r#"{"type":"submitted","request_id":2,"ticket":{"batch_id":1,"jobs":[{"job_id":1,"input":"C:\\영상\\발표 녹화.mov","to":"mp4"}],"rejected":[]}}"#,
        r#"{"type":"ack","request_id":3,"ok":true}"#,
        r#"{"type":"error","request_id":3,"error":{"code":"invalid_input","message":"…","engine":null}}"#,
        r#"{"type":"event","event":{"type":"progress","batch_id":1,"job_id":1,"progress":0.42}}"#,
    ];

    fn same_json(a: &str, b: &str) {
        let va: Value = serde_json::from_str(a).expect("a");
        let vb: Value = serde_json::from_str(b).expect("b");
        assert_eq!(va, vb, "\n원본: {a}\n결과: {b}");
    }

    #[test]
    fn client_examples_round_trip() {
        for line in CLIENT_LINES {
            let msg = decode_client(line).expect(line);
            let out = encode_line(&msg).expect("인코딩");
            assert!(out.ends_with('\n'));
            assert_eq!(out.matches('\n').count(), 1);
            same_json(line, out.trim_end());
            assert_eq!(decode_client(&out).expect("다시 읽기"), msg);
        }
    }

    #[test]
    fn server_examples_round_trip() {
        for line in SERVER_LINES {
            let msg = decode_server(line).expect(line);
            let out = encode_line(&msg).expect("인코딩");
            same_json(line, out.trim_end());
            assert_eq!(decode_server(&out).expect("다시 읽기"), msg);
        }
    }

    #[test]
    fn client_examples_decode_to_expected_values() {
        assert_eq!(
            decode_client(CLIENT_LINES[0]).expect("hello"),
            ClientMessage::Hello {
                protocol: PROTOCOL_VERSION,
                client: ClientKind::Shell,
                pid: 4120
            }
        );
        let ClientMessage::Submit {
            request_id,
            request,
        } = decode_client(CLIENT_LINES[2]).expect("submit")
        else {
            panic!("submit이어야 함");
        };
        assert_eq!(request_id, 2);
        assert_eq!(request.to, "mp4");
        assert_eq!(request.preset, PresetId::Web);
        assert_eq!(request.files, vec![PathBuf::from(r"C:\영상\발표 녹화.mov")]);
    }

    #[test]
    fn exact_encoding_of_server_messages() {
        let ticket = ServerMessage::Submitted {
            request_id: 2,
            ticket: BatchTicket {
                batch_id: 1,
                jobs: vec![QueuedJob {
                    job_id: 1,
                    input: PathBuf::from(r"C:\영상\발표 녹화.mov"),
                    to: "mp4".into(),
                }],
                rejected: vec![],
            },
        };
        assert_eq!(
            encode_line(&ticket).expect("인코딩"),
            format!("{}\n", SERVER_LINES[1])
        );
        let err = ServerMessage::Error {
            request_id: Some(3),
            error: ErrorInfo {
                code: ErrorCode::InvalidInput,
                message: "…".into(),
                engine: None,
            },
        };
        assert_eq!(
            encode_line(&err).expect("인코딩"),
            format!("{}\n", SERVER_LINES[3])
        );
        let ev = ServerMessage::Event {
            event: JobEvent::Progress {
                batch_id: 1,
                job_id: 1,
                progress: Some(0.42),
            },
        };
        assert_eq!(
            encode_line(&ev).expect("인코딩"),
            format!("{}\n", SERVER_LINES[4])
        );
        let hello = ServerMessage::HelloAck {
            protocol: PROTOCOL_VERSION,
            server_version: crate::VERSION.into(),
        };
        assert_eq!(
            encode_line(&hello).expect("인코딩"),
            format!("{}\n", SERVER_LINES[0])
        );
    }

    #[test]
    fn formats_message_round_trip() {
        let report = FormatsReport {
            files: vec![FileFormat {
                path: PathBuf::from(r"C:\영상\발표 녹화.mov"),
                format: "mov".into(),
                category: Category::Video,
            }],
            skipped: vec![SkippedFile {
                path: PathBuf::from(r"C:\문서\보고서.docx"),
                reason: crate::formats::SkipReason::DifferentCategory,
            }],
            options: vec![FormatOption {
                id: "mp4".into(),
                label: "MP4".into(),
                description: "영상 (H.264)".into(),
                engines: vec![EngineId::Ffmpeg],
                available: true,
                unavailable: None,
            }],
            layout: RadialLayout {
                slot_count: 4,
                slots: vec![
                    RadialSlot::Format {
                        format_id: "mp4".into(),
                    },
                    RadialSlot::Empty,
                    RadialSlot::Empty,
                    RadialSlot::Empty,
                ],
                more: vec![],
            },
        };
        let msg = ServerMessage::Formats {
            request_id: 1,
            report,
        };
        let line = encode_line(&msg).expect("인코딩");
        let v: Value = serde_json::from_str(line.trim_end()).expect("json");
        assert_eq!(v["type"], "formats");
        assert_eq!(v["request_id"], 1);
        assert_eq!(v["report"]["layout"]["slots"][0]["kind"], "format");
        assert_eq!(v["report"]["options"][0]["unavailable"], Value::Null);
        assert_eq!(decode_server(&line).expect("다시 읽기"), msg);
    }

    #[test]
    fn decode_accepts_crlf_and_rejects_bad_input() {
        let msg = decode_client("{\"type\":\"subscribe\",\"request_id\":4}\r\n").expect("crlf");
        assert_eq!(msg, ClientMessage::Subscribe { request_id: 4 });

        let e = decode_client("{\"type\":\"nope\"}").expect_err("모르는 type");
        assert_eq!(e.code(), ErrorCode::InvalidInput);
        let e = decode_client("not json").expect_err("json 아님");
        assert_eq!(e.code(), ErrorCode::InvalidInput);
        // protocol 누락
        let e = decode_client(r#"{"type":"hello","client":"cli","pid":1}"#).expect_err("필드 누락");
        assert_eq!(e.code(), ErrorCode::InvalidInput);

        let huge = format!(
            "{{\"type\":\"show_main_window\",\"request_id\":1,\"files\":[\"{}\"]}}",
            "a".repeat(MAX_LINE_BYTES)
        );
        let e = decode_client(&huge).expect_err("길이 초과");
        assert_eq!(e.code(), ErrorCode::InvalidInput);
        let e = decode_server(&huge).expect_err("길이 초과");
        assert_eq!(e.code(), ErrorCode::InvalidInput);
    }

    #[test]
    fn encode_rejects_oversized() {
        let msg = ClientMessage::ShowMainWindow {
            request_id: 1,
            files: vec![PathBuf::from("a".repeat(MAX_LINE_BYTES))],
        };
        let e = encode_line(&msg).expect_err("길이 초과");
        assert_eq!(e.code(), ErrorCode::InvalidInput);
    }

    #[test]
    fn pipe_constants() {
        assert_eq!(PIPE_NAME, r"\\.\pipe\convtool");
        assert_eq!(PROTOCOL_VERSION, 1);
        assert_eq!(MAX_LINE_BYTES, 1_048_576);
    }
}
