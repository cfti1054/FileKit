//! 프리셋(web/original/small)과 엔진별 매개변수.
//!
//! 값은 설계 계획의 표(가정)를 따른 상수다. 바꾸기 쉽게 한곳에 둔다.

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetId {
    #[default]
    Web,
    Original,
    Small,
}

impl PresetId {
    pub const ALL: [PresetId; 3] = [PresetId::Web, PresetId::Original, PresetId::Small];

    /// 직렬화 문자열과 같은 ID ("web" | "original" | "small")
    pub fn as_str(self) -> &'static str {
        match self {
            PresetId::Web => "web",
            PresetId::Original => "original",
            PresetId::Small => "small",
        }
    }
}

impl std::fmt::Display for PresetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PresetId {
    type Err = CoreError;

    /// "web" | "original" | "small"
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "web" => Ok(PresetId::Web),
            "original" => Ok(PresetId::Original),
            "small" => Ok(PresetId::Small),
            other => Err(CoreError::InvalidInput(format!(
                "알 수 없는 프리셋입니다: {other} (web, original, small 중 하나)"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetInfo {
    pub id: PresetId,
    pub label: String,
    pub description: String,
}

pub fn list() -> Vec<PresetInfo> {
    PresetId::ALL
        .iter()
        .map(|&id| {
            let (label, description) = match id {
                PresetId::Web => ("웹용", "공유하기 좋은 크기와 화질 (기본)"),
                PresetId::Original => ("원본 화질", "크기를 줄이지 않고 화질을 최대한 유지"),
                PresetId::Small => ("작은 크기", "화질을 낮춰 파일 크기를 줄임"),
            };
            PresetInfo {
                id,
                label: label.into(),
                description: description.into(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoParams {
    pub max_height: Option<u32>,
    pub video_kbps: Option<u32>,
    pub vp9_crf: u8,
    pub audio_kbps: u32,
    pub gif_fps: u32,
    pub gif_width: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioParams {
    pub mp3_vbr_q: Option<u8>,
    pub mp3_kbps: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageParams {
    pub quality: u8,
    pub max_dim: Option<u32>,
    pub png_compression: u8,
}

pub fn video(p: PresetId) -> VideoParams {
    match p {
        PresetId::Web => VideoParams {
            max_height: Some(1080),
            video_kbps: Some(5000),
            vp9_crf: 32,
            audio_kbps: 128,
            gif_fps: 12,
            gif_width: 480,
        },
        PresetId::Original => VideoParams {
            max_height: None,
            video_kbps: Some(12000),
            vp9_crf: 24,
            audio_kbps: 192,
            gif_fps: 15,
            gif_width: 720,
        },
        PresetId::Small => VideoParams {
            max_height: Some(720),
            video_kbps: Some(1500),
            vp9_crf: 38,
            audio_kbps: 96,
            gif_fps: 10,
            gif_width: 320,
        },
    }
}

pub fn audio(p: PresetId) -> AudioParams {
    match p {
        PresetId::Web => AudioParams {
            mp3_vbr_q: Some(2),
            mp3_kbps: None,
        },
        PresetId::Original => AudioParams {
            mp3_vbr_q: None,
            mp3_kbps: Some(320),
        },
        PresetId::Small => AudioParams {
            mp3_vbr_q: Some(6),
            mp3_kbps: None,
        },
    }
}

pub fn image(p: PresetId) -> ImageParams {
    match p {
        PresetId::Web => ImageParams {
            quality: 82,
            max_dim: Some(2560),
            png_compression: 6,
        },
        PresetId::Original => ImageParams {
            quality: 95,
            max_dim: None,
            png_compression: 6,
        },
        PresetId::Small => ImageParams {
            quality: 70,
            max_dim: Some(1280),
            png_compression: 9,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_from_str_and_serde() {
        assert_eq!("web".parse::<PresetId>().expect("web"), PresetId::Web);
        assert_eq!(
            "original".parse::<PresetId>().expect("original"),
            PresetId::Original
        );
        assert_eq!("small".parse::<PresetId>().expect("small"), PresetId::Small);
        assert!("Web".parse::<PresetId>().is_err());
        assert_eq!(PresetId::default(), PresetId::Web);
        for id in PresetId::ALL {
            let s = serde_json::to_string(&id).expect("직렬화");
            assert_eq!(s, format!("\"{}\"", id.as_str()));
        }
    }

    #[test]
    fn list_json_shape() {
        let v = serde_json::to_value(list()).expect("직렬화");
        let arr = v.as_array().expect("배열");
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["id"], "web");
        assert!(arr[0]["label"].is_string());
        assert!(arr[0]["description"].is_string());
    }
}
