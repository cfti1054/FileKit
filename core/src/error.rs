//! 코어 오류 타입과 경계용 오류 정보.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engines::EngineId;

/// 경계(CLI JSON, 파이프, Tauri)에서 쓰는 오류 분류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedFormat,
    EngineMissing,
    CodecMissing,
    EngineFailed,
    Canceled,
    Io,
    InvalidInput,
    Internal,
}

impl ErrorCode {
    /// CLI 종료 코드 매핑: unsupported_format→2, engine_missing|codec_missing→3, 그 밖→1
    pub fn cli_exit_code(self) -> u8 {
        match self {
            ErrorCode::UnsupportedFormat => 2,
            ErrorCode::EngineMissing | ErrorCode::CodecMissing => 3,
            ErrorCode::EngineFailed
            | ErrorCode::Canceled
            | ErrorCode::Io
            | ErrorCode::InvalidInput
            | ErrorCode::Internal => 1,
        }
    }
}

/// 어느 경로에서나 오류는 이 형태 하나로 나간다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub message: String,
    pub engine: Option<EngineId>,
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("지원하지 않는 형식입니다: {input} → {}", .to.as_deref().unwrap_or("?"))]
    UnsupportedFormat { input: String, to: Option<String> },
    #[error("{0} 엔진을 찾을 수 없습니다")]
    EngineMissing(EngineId),
    #[error("{engine}에 {codec} 인코더가 없습니다")]
    CodecMissing { engine: EngineId, codec: String },
    #[error("{engine} 실행 실패 (종료 코드 {code:?}): {stderr_tail}")]
    EngineFailed {
        engine: EngineId,
        code: Option<i32>,
        stderr_tail: String,
    },
    #[error("취소되었습니다")]
    Canceled,
    #[error("파일 작업 실패 ({path}): {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("잘못된 입력: {0}")]
    InvalidInput(String),
    #[error("내부 오류: {0}")]
    Internal(String),
}

impl CoreError {
    pub fn code(&self) -> ErrorCode {
        match self {
            CoreError::UnsupportedFormat { .. } => ErrorCode::UnsupportedFormat,
            CoreError::EngineMissing(_) => ErrorCode::EngineMissing,
            CoreError::CodecMissing { .. } => ErrorCode::CodecMissing,
            CoreError::EngineFailed { .. } => ErrorCode::EngineFailed,
            CoreError::Canceled => ErrorCode::Canceled,
            CoreError::Io { .. } => ErrorCode::Io,
            CoreError::InvalidInput(_) => ErrorCode::InvalidInput,
            CoreError::Internal(_) => ErrorCode::Internal,
        }
    }

    /// message = to_string(), engine = 엔진 관련 오류일 때만
    pub fn info(&self) -> ErrorInfo {
        let engine = match self {
            CoreError::EngineMissing(e) => Some(*e),
            CoreError::CodecMissing { engine, .. } | CoreError::EngineFailed { engine, .. } => {
                Some(*engine)
            }
            _ => None,
        };
        ErrorInfo {
            code: self.code(),
            message: self.to_string(),
            engine,
        }
    }
}

impl From<CoreError> for ErrorInfo {
    fn from(e: CoreError) -> Self {
        e.info()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_exit_codes() {
        assert_eq!(ErrorCode::UnsupportedFormat.cli_exit_code(), 2);
        assert_eq!(ErrorCode::EngineMissing.cli_exit_code(), 3);
        assert_eq!(ErrorCode::CodecMissing.cli_exit_code(), 3);
        for c in [
            ErrorCode::EngineFailed,
            ErrorCode::Canceled,
            ErrorCode::Io,
            ErrorCode::InvalidInput,
            ErrorCode::Internal,
        ] {
            assert_eq!(c.cli_exit_code(), 1, "{c:?}");
        }
    }

    #[test]
    fn error_code_serde_snake_case() {
        let s = serde_json::to_string(&ErrorCode::UnsupportedFormat).expect("직렬화");
        assert_eq!(s, "\"unsupported_format\"");
        let c: ErrorCode = serde_json::from_str("\"codec_missing\"").expect("역직렬화");
        assert_eq!(c, ErrorCode::CodecMissing);
    }

    #[test]
    fn unsupported_format_message_matches_cli_example() {
        let e = CoreError::UnsupportedFormat {
            input: "xyz".into(),
            to: Some("mp4".into()),
        };
        let info = e.info();
        let json = serde_json::to_value(&info).expect("직렬화");
        assert_eq!(
            json,
            serde_json::json!({
                "code": "unsupported_format",
                "message": "지원하지 않는 형식입니다: xyz → mp4",
                "engine": null
            })
        );
        let e = CoreError::UnsupportedFormat {
            input: "xyz".into(),
            to: None,
        };
        assert_eq!(e.to_string(), "지원하지 않는 형식입니다: xyz → ?");
    }

    #[test]
    fn info_carries_engine() {
        let e = CoreError::EngineMissing(EngineId::Office);
        let info = e.info();
        assert_eq!(info.code, ErrorCode::EngineMissing);
        assert_eq!(info.engine, Some(EngineId::Office));
        assert_eq!(info.message, "LibreOffice 엔진을 찾을 수 없습니다");

        let e = CoreError::Io {
            path: PathBuf::from(r"C:\영상\발표 녹화.mov"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "없음"),
        };
        assert_eq!(e.code(), ErrorCode::Io);
        assert!(e.to_string().contains("발표 녹화.mov"));
        assert_eq!(e.info().engine, None);
    }
}
