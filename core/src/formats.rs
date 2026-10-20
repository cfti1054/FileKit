//! 형식 표, 출력 목록, 공통 형식(교집합), 기본 방향 배치, 메뉴용 질의.
//!
//! 형식 ID는 점 없는 소문자 확장자 문자열이다(`"mp4"`). 출력 순서는 "자주 쓰는 순"이다(표 순서).
//! 항등 변환(mp4→mp4 등)은 지원하지 않는 것으로 친다.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engines::{EngineId, EngineSet};
use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Video,
    Audio,
    Image,
    Document,
    Text,
}

/// 정적 형식 표 항목 (직렬화하지 않음)
#[derive(Debug, PartialEq, Eq)]
pub struct OutputFormat {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// 순서대로 실행. 예: md→pdf = [Pandoc, Office]
    pub engines: &'static [EngineId],
}

// ===== 형식 표 =====

const FFMPEG: &[EngineId] = &[EngineId::Ffmpeg];
const VIPS: &[EngineId] = &[EngineId::Vips];
const OFFICE: &[EngineId] = &[EngineId::Office];
const PANDOC: &[EngineId] = &[EngineId::Pandoc];
const PANDOC_OFFICE: &[EngineId] = &[EngineId::Pandoc, EngineId::Office];

static VIDEO_OUTPUTS: [OutputFormat; 4] = [
    OutputFormat {
        id: "mp4",
        label: "MP4",
        description: "영상 (H.264)",
        engines: FFMPEG,
    },
    OutputFormat {
        id: "mp3",
        label: "MP3",
        description: "음성 추출",
        engines: FFMPEG,
    },
    OutputFormat {
        id: "gif",
        label: "GIF",
        description: "움직이는 이미지",
        engines: FFMPEG,
    },
    OutputFormat {
        id: "webm",
        label: "WEBM",
        description: "영상 (VP9)",
        engines: FFMPEG,
    },
];

static AUDIO_OUTPUTS: [OutputFormat; 3] = [
    OutputFormat {
        id: "mp3",
        label: "MP3",
        description: "음성",
        engines: FFMPEG,
    },
    OutputFormat {
        id: "wav",
        label: "WAV",
        description: "무손실 음성",
        engines: FFMPEG,
    },
    OutputFormat {
        id: "flac",
        label: "FLAC",
        description: "무손실 압축 음성",
        engines: FFMPEG,
    },
];

static IMAGE_OUTPUTS: [OutputFormat; 3] = [
    OutputFormat {
        id: "jpg",
        label: "JPG",
        description: "사진",
        engines: VIPS,
    },
    OutputFormat {
        id: "png",
        label: "PNG",
        description: "무손실 이미지",
        engines: VIPS,
    },
    OutputFormat {
        id: "webp",
        label: "WEBP",
        description: "웹용 이미지",
        engines: VIPS,
    },
];

static DOCUMENT_OUTPUTS: [OutputFormat; 2] = [
    OutputFormat {
        id: "pdf",
        label: "PDF",
        description: "PDF 문서",
        engines: OFFICE,
    },
    OutputFormat {
        id: "txt",
        label: "TXT",
        description: "텍스트만",
        engines: OFFICE,
    },
];

static TEXT_OUTPUTS: [OutputFormat; 3] = [
    OutputFormat {
        id: "pdf",
        label: "PDF",
        description: "PDF 문서",
        engines: PANDOC_OFFICE,
    },
    OutputFormat {
        id: "docx",
        label: "DOCX",
        description: "Word 문서",
        engines: PANDOC,
    },
    OutputFormat {
        id: "html",
        label: "HTML",
        description: "웹 페이지",
        engines: PANDOC,
    },
];

/// 지원하는 입력 형식 (정규화된 ID, 분류)
static INPUTS: [(&str, Category); 25] = [
    ("mp4", Category::Video),
    ("mov", Category::Video),
    ("avi", Category::Video),
    ("mkv", Category::Video),
    ("webm", Category::Video),
    ("mp3", Category::Audio),
    ("wav", Category::Audio),
    ("m4a", Category::Audio),
    ("flac", Category::Audio),
    ("aac", Category::Audio),
    ("ogg", Category::Audio),
    ("jpg", Category::Image),
    ("png", Category::Image),
    ("webp", Category::Image),
    ("heic", Category::Image),
    ("bmp", Category::Image),
    ("tiff", Category::Image),
    ("gif", Category::Image),
    ("docx", Category::Document),
    ("pptx", Category::Document),
    ("xlsx", Category::Document),
    ("odt", Category::Document),
    ("rtf", Category::Document),
    ("md", Category::Text),
    ("html", Category::Text),
];

/// 별칭 → 정규화된 ID
static ALIASES: [(&str, &str); 5] = [
    ("jpeg", "jpg"),
    ("heif", "heic"),
    ("tif", "tiff"),
    ("htm", "html"),
    ("markdown", "md"),
];

fn outputs_of(category: Category) -> &'static [OutputFormat] {
    match category {
        Category::Video => &VIDEO_OUTPUTS,
        Category::Audio => &AUDIO_OUTPUTS,
        Category::Image => &IMAGE_OUTPUTS,
        Category::Document => &DOCUMENT_OUTPUTS,
        Category::Text => &TEXT_OUTPUTS,
    }
}

fn all_outputs() -> impl Iterator<Item = &'static OutputFormat> {
    VIDEO_OUTPUTS
        .iter()
        .chain(AUDIO_OUTPUTS.iter())
        .chain(IMAGE_OUTPUTS.iter())
        .chain(DOCUMENT_OUTPUTS.iter())
        .chain(TEXT_OUTPUTS.iter())
}

/// 형식 표에서 빼는 개별 조합. pptx→txt: Impress에는 텍스트 내보내기 필터가 없다(설계 계획 확인 필요 2).
fn excluded(input: &str, to: &str) -> bool {
    input == "pptx" && to == "txt"
}

/// "JPEG"→"jpg", "tif"→"tiff", "htm"→"html", "markdown"→"md"
///
/// 입력이나 출력으로 알려진 형식이면 정규화된 ID를 돌려준다(앞의 '.' 하나는 무시한다).
/// 입력으로 지원하는지는 [`category_of`]·[`format_of_path`]로 판단한다.
pub fn normalize_ext(ext: &str) -> Option<&'static str> {
    let lower = ext.strip_prefix('.').unwrap_or(ext).to_ascii_lowercase();
    let id = ALIASES
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map_or(lower.as_str(), |(_, id)| id);
    INPUTS
        .iter()
        .map(|(i, _)| *i)
        .chain(all_outputs().map(|o| o.id))
        .find(|known| *known == id)
}

/// 지원하는 입력이면 정규화된 ID
pub fn format_of_path(path: &Path) -> Option<&'static str> {
    let id = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(normalize_ext)?;
    category_of(id).map(|_| id)
}

pub fn category_of(input: &str) -> Option<Category> {
    let id = normalize_ext(input)?;
    INPUTS.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
}

/// 출력으로 알려진 형식인가 (요청 검증용)
pub(crate) fn is_known_output(to: &str) -> bool {
    normalize_ext(to).is_some_and(|id| all_outputs().any(|o| o.id == id))
}

/// 단일 입력의 출력 목록. 같은 형식으로의 변환(항등)은 뺀다. 순서 = "자주 쓰는 순" 표 순서.
pub fn outputs_for(input: &str) -> Result<Vec<&'static OutputFormat>, CoreError> {
    let unsupported = || CoreError::UnsupportedFormat {
        input: input.to_string(),
        to: None,
    };
    let id = normalize_ext(input).ok_or_else(unsupported)?;
    let category = category_of(id).ok_or_else(unsupported)?;
    Ok(outputs_of(category)
        .iter()
        .filter(|o| o.id != id && !excluded(id, o.id))
        .collect())
}

/// 여러 입력 형식의 출력 집합 교집합. 순서는 첫 입력 기준. 빈 결과는 Ok(vec![]).
pub fn common_outputs(inputs: &[&str]) -> Result<Vec<&'static OutputFormat>, CoreError> {
    let Some((first, rest)) = inputs.split_first() else {
        return Ok(Vec::new());
    };
    let mut result = outputs_for(first)?;
    for input in rest {
        let others = outputs_for(input)?;
        // 같은 id라도 분류에 따라 엔진·설명이 다를 수 있다(예: video→mp3, audio→mp3).
        // 첫 입력 기준 항목을 남긴다.
        result.retain(|o| others.iter().any(|x| x.id == o.id));
    }
    Ok(result)
}

pub fn is_supported(input: &str, to: &str) -> bool {
    engines_for(input, to).is_some()
}

pub fn engines_for(input: &str, to: &str) -> Option<&'static [EngineId]> {
    let to = normalize_ext(to)?;
    outputs_for(input)
        .ok()?
        .into_iter()
        .find(|o| o.id == to)
        .map(|o| o.engines)
}

/// 표의 출력 항목 (정규화된 입력·출력 ID로)
pub(crate) fn output_entry(input: &str, to: &str) -> Option<&'static OutputFormat> {
    let to = normalize_ext(to)?;
    outputs_for(input).ok()?.into_iter().find(|o| o.id == to)
}

// ===== 가용성 =====

/// 출력 하나를 지금 만들 수 없는 이유. 메뉴 표시용(`Unavailable`)과 작업 거절용(`CoreError`)을 함께 만든다.
pub(crate) struct Problem {
    pub unavailable: Unavailable,
    pub error: CoreError,
}

fn engine_missing_message(id: EngineId) -> String {
    let particle = match id {
        EngineId::Ffmpeg | EngineId::Pandoc => "이",
        EngineId::Vips | EngineId::Office => "가",
    };
    format!("{}{particle} 설치되지 않았습니다", id.display_name())
}

fn codec_problem(engine: EngineId, codec: &str, message: String) -> Problem {
    Problem {
        unavailable: Unavailable {
            code: UnavailableCode::CodecMissing,
            engine: Some(engine),
            message,
        },
        error: CoreError::CodecMissing {
            engine,
            codec: codec.to_string(),
        },
    }
}

/// 가용성 판정:
/// - 필요한 엔진 중 하나라도 없으면 engine_missing
/// - heic 입력이고 `vips_heic() == Some(false)`이면 codec_missing
/// - ffmpeg mp4: h264 없음, webm: vp9·opus 중 하나라도 없음, mp3: mp3lame 없음 → codec_missing.
///   caps를 아직 probe하지 않았으면 가능으로 본다.
pub(crate) fn availability_problem(
    input_formats: &[&str],
    out: &OutputFormat,
    engines: &EngineSet,
) -> Option<Problem> {
    if let Some(&missing) = out.engines.iter().find(|&&e| !engines.is_available(e)) {
        return Some(Problem {
            unavailable: Unavailable {
                code: UnavailableCode::EngineMissing,
                engine: Some(missing),
                message: engine_missing_message(missing),
            },
            error: CoreError::EngineMissing(missing),
        });
    }
    let has_heic = input_formats
        .iter()
        .any(|f| normalize_ext(f) == Some("heic"));
    if has_heic && engines.vips_heic() == Some(false) {
        return Some(codec_problem(
            EngineId::Vips,
            "HEIC(heifload)",
            "이 libvips 빌드는 HEIC를 읽지 못합니다".into(),
        ));
    }
    if out.engines.contains(&EngineId::Ffmpeg) {
        if let Some(caps) = engines.ffmpeg_caps() {
            match out.id {
                "mp4" if caps.h264.is_none() => {
                    return Some(codec_problem(
                        EngineId::Ffmpeg,
                        "H.264(h264_mf, libopenh264)",
                        "FFmpeg에 H.264 인코더(h264_mf, libopenh264)가 없습니다".into(),
                    ));
                }
                "webm" if !(caps.vp9 && caps.opus) => {
                    return Some(codec_problem(
                        EngineId::Ffmpeg,
                        "VP9/Opus(libvpx-vp9, libopus)",
                        "FFmpeg에 VP9/Opus 인코더(libvpx-vp9, libopus)가 없습니다".into(),
                    ));
                }
                "mp3" if !caps.mp3lame => {
                    return Some(codec_problem(
                        EngineId::Ffmpeg,
                        "MP3(libmp3lame)",
                        "FFmpeg에 MP3 인코더(libmp3lame)가 없습니다".into(),
                    ));
                }
                _ => {}
            }
        }
    }
    None
}

// ===== 기본 방향 배치 =====

/// 기본 칸 수
pub const DEFAULT_SLOT_COUNT: u8 = 4;

/// "더 보기" 칸 번호: θ=135°가 들어 있는 칸.
///
/// 칸 i = [(270 + i·360/N) mod 360, +360/N) 이므로 135°는 270°에서 시계 방향으로 225° 떨어져 있다.
/// i = floor(225·N/360). N=4 → 2, N=8 → 5. 시작 경계 포함 규약과 맞는다(N=8에서 정확히 135°가 5번 칸 시작).
pub fn more_slot_index(slot_count: u8) -> usize {
    let n = usize::from(slot_count.clamp(4, 8));
    225 * n / 360
}

/// 기본 방향 배치. option_ids는 우선순위 순서.
///
/// 칸 번호 규약: 12시=0°, 시계 방향. 칸 i = [(270 + i·360/N) mod 360, +360/N).
/// "더 보기" 칸은 θ=135°가 들어 있는 칸 (N=4 → 2, N=8 → 5).
///
/// spec 5장: 더 보기 칸(N=4에서 오른쪽 아래)은 형식 칸으로 쓰지 않는다.
///
/// - 항목 수 ≤ N-1: 더 보기 칸을 뺀 칸에 0번부터 차례로 놓는다(N=4: 0,1,3).
///   남는 칸(더 보기 칸 포함)은 `Empty`. `more`는 비어 있다.
/// - 항목 수 > N-1: 처음 N-1개를 같은 순서로 놓고, 더 보기 칸에 `More`, 나머지는 `more`에.
///   N=4에서 항목 4개면 형식 3개 + `More`(more = 1개).
///
/// slot_count는 4..=8로 자른다.
pub fn default_layout(option_ids: &[&str], slot_count: u8) -> RadialLayout {
    let slot_count = slot_count.clamp(4, 8);
    let n = usize::from(slot_count);
    let more_idx = more_slot_index(slot_count);
    let format_slots = (0..n).filter(|&i| i != more_idx);

    let mut slots = vec![RadialSlot::Empty; n];
    let split = option_ids.len().min(n - 1);
    let (front, rest) = option_ids.split_at(split);
    for (slot, id) in format_slots.zip(front) {
        slots[slot] = RadialSlot::Format {
            format_id: (*id).to_string(),
        };
    }
    if !rest.is_empty() {
        slots[more_idx] = RadialSlot::More;
    }
    let more = rest.iter().map(|s| (*s).to_string()).collect();
    RadialLayout {
        slot_count,
        slots,
        more,
    }
}

// ===== 메뉴용 질의 =====

/// 메뉴용 질의. 규칙:
/// 1) 지원하지 않는 확장자 → skipped(unsupported_format)
/// 2) primary가 있으면 primary와 다른 분류 → skipped(different_category)   (F-03 "같은 종류")
/// 3) options = 남은 입력 형식들의 common_outputs, 엔진 가용성 표시
/// 4) 남은 파일이 0개이거나 options가 비면 Err(UnsupportedFormat)
///
/// 파일 존재 여부는 확인하지 않는다(확장자만). primary 자체가 지원하지 않는 형식이면
/// 분류 걸러내기를 하지 않는다(규칙 1이 그 파일을 뺀다). 배치는 기본 4칸이다.
pub fn query_formats(
    files: &[PathBuf],
    primary: Option<&Path>,
    engines: &EngineSet,
) -> Result<FormatsReport, CoreError> {
    let primary_category = primary.and_then(format_of_path).and_then(category_of);

    let mut kept = Vec::new();
    let mut skipped = Vec::new();
    for path in files {
        let Some(format) = format_of_path(path) else {
            skipped.push(SkippedFile {
                path: path.clone(),
                reason: SkipReason::UnsupportedFormat,
            });
            continue;
        };
        let Some(category) = category_of(format) else {
            skipped.push(SkippedFile {
                path: path.clone(),
                reason: SkipReason::UnsupportedFormat,
            });
            continue;
        };
        if primary_category.is_some_and(|pc| pc != category) {
            skipped.push(SkippedFile {
                path: path.clone(),
                reason: SkipReason::DifferentCategory,
            });
            continue;
        }
        kept.push((path.clone(), format, category));
    }

    if kept.is_empty() {
        return Err(CoreError::UnsupportedFormat {
            input: describe_exts(files),
            to: None,
        });
    }

    // 입력 형식 (첫 등장 순서, 중복 제거)
    let mut formats: Vec<&'static str> = Vec::new();
    for (_, f, _) in &kept {
        if !formats.contains(f) {
            formats.push(f);
        }
    }

    let outputs = common_outputs(&formats)?;
    if outputs.is_empty() {
        return Err(CoreError::UnsupportedFormat {
            input: formats.join(", "),
            to: None,
        });
    }

    let options: Vec<FormatOption> = outputs
        .iter()
        .map(|o| {
            let problem = availability_problem(&formats, o, engines);
            FormatOption {
                id: o.id.to_string(),
                label: o.label.to_string(),
                description: o.description.to_string(),
                engines: o.engines.to_vec(),
                available: problem.is_none(),
                unavailable: problem.map(|p| p.unavailable),
            }
        })
        .collect();
    let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
    let layout = default_layout(&ids, DEFAULT_SLOT_COUNT);

    Ok(FormatsReport {
        files: kept
            .into_iter()
            .map(|(path, format, category)| FileFormat {
                path,
                format: format.to_string(),
                category,
            })
            .collect(),
        skipped,
        options,
        layout,
    })
}

/// 오류 메시지용 확장자 목록 ("xyz", "xyz, abc", 확장자 없음 → "?")
fn describe_exts(files: &[PathBuf]) -> String {
    let set: BTreeSet<String> = files
        .iter()
        .map(|p| {
            p.extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_else(|| "?".into())
        })
        .collect();
    if set.is_empty() {
        "?".into()
    } else {
        set.into_iter().collect::<Vec<_>>().join(", ")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatsReport {
    pub files: Vec<FileFormat>,
    pub skipped: Vec<SkippedFile>,
    pub options: Vec<FormatOption>,
    pub layout: RadialLayout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileFormat {
    pub path: PathBuf,
    pub format: String,
    pub category: Category,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkippedFile {
    pub path: PathBuf,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    UnsupportedFormat,
    DifferentCategory,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormatOption {
    pub id: String,
    pub label: String,
    pub description: String,
    pub engines: Vec<EngineId>,
    pub available: bool,
    pub unavailable: Option<Unavailable>,
}

/// message 예: "LibreOffice가 설치되지 않았습니다", "이 libvips 빌드는 HEIC를 읽지 못합니다",
/// "FFmpeg에 H.264 인코더(h264_mf, libopenh264)가 없습니다"
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unavailable {
    pub code: UnavailableCode,
    pub engine: Option<EngineId>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableCode {
    EngineMissing,
    CodecMissing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadialLayout {
    /// 4..=8, 기본 4
    pub slot_count: u8,
    /// len == slot_count, 인덱스 = 칸(섹터) 번호
    pub slots: Vec<RadialSlot>,
    /// 두 번째 고리에 놓일 형식 ID (우선순위 순)
    pub more: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RadialSlot {
    Format { format_id: String },
    More,
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::ffmpeg::{FfmpegCaps, H264Encoder};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn sample_report() -> FormatsReport {
        FormatsReport {
            files: vec![FileFormat {
                path: PathBuf::from(r"C:\영상\발표 녹화.mov"),
                format: "mov".into(),
                category: Category::Video,
            }],
            skipped: vec![SkippedFile {
                path: PathBuf::from(r"C:\영상\메모.xyz"),
                reason: SkipReason::UnsupportedFormat,
            }],
            options: vec![
                FormatOption {
                    id: "mp4".into(),
                    label: "MP4".into(),
                    description: "영상 (H.264)".into(),
                    engines: vec![EngineId::Ffmpeg],
                    available: false,
                    unavailable: Some(Unavailable {
                        code: UnavailableCode::CodecMissing,
                        engine: Some(EngineId::Ffmpeg),
                        message: "FFmpeg에 H.264 인코더(h264_mf, libopenh264)가 없습니다".into(),
                    }),
                },
                FormatOption {
                    id: "mp3".into(),
                    label: "MP3".into(),
                    description: "음성 추출".into(),
                    engines: vec![EngineId::Ffmpeg],
                    available: true,
                    unavailable: None,
                },
            ],
            // 직렬화 모양 예시(default_layout 결과와 일치할 필요는 없음). More는 규약대로 N=4 → 2번 칸.
            layout: RadialLayout {
                slot_count: 4,
                slots: vec![
                    RadialSlot::Format {
                        format_id: "mp4".into(),
                    },
                    RadialSlot::Format {
                        format_id: "mp3".into(),
                    },
                    RadialSlot::More,
                    RadialSlot::Empty,
                ],
                more: vec!["webm".into()],
            },
        }
    }

    #[test]
    fn formats_report_json_shape() {
        let v = serde_json::to_value(sample_report()).expect("직렬화");
        assert_eq!(
            v,
            json!({
                "files": [{"path": "C:\\영상\\발표 녹화.mov", "format": "mov", "category": "video"}],
                "skipped": [{"path": "C:\\영상\\메모.xyz", "reason": "unsupported_format"}],
                "options": [
                    {"id": "mp4", "label": "MP4", "description": "영상 (H.264)", "engines": ["ffmpeg"],
                     "available": false,
                     "unavailable": {"code": "codec_missing", "engine": "ffmpeg",
                                     "message": "FFmpeg에 H.264 인코더(h264_mf, libopenh264)가 없습니다"}},
                    {"id": "mp3", "label": "MP3", "description": "음성 추출", "engines": ["ffmpeg"],
                     "available": true, "unavailable": null}
                ],
                "layout": {
                    "slot_count": 4,
                    "slots": [
                        {"kind": "format", "format_id": "mp4"},
                        {"kind": "format", "format_id": "mp3"},
                        {"kind": "more"},
                        {"kind": "empty"}
                    ],
                    "more": ["webm"]
                }
            })
        );
        let back: FormatsReport = serde_json::from_value(v).expect("역직렬화");
        assert_eq!(back, sample_report());
    }

    #[test]
    fn enums_snake_case() {
        assert_eq!(
            serde_json::to_value(SkipReason::DifferentCategory).expect("직렬화"),
            json!("different_category")
        );
        assert_eq!(
            serde_json::to_value(Category::Document).expect("직렬화"),
            json!("document")
        );
        assert_eq!(
            serde_json::to_value(UnavailableCode::EngineMissing).expect("직렬화"),
            json!("engine_missing")
        );
    }

    // ----- 형식 표 -----

    fn ids(v: &[&'static OutputFormat]) -> Vec<&'static str> {
        v.iter().map(|o| o.id).collect()
    }

    #[test]
    fn input_table_all_formats_and_aliases() {
        let expected: [(&str, Category); 25] = [
            ("mp4", Category::Video),
            ("mov", Category::Video),
            ("avi", Category::Video),
            ("mkv", Category::Video),
            ("webm", Category::Video),
            ("mp3", Category::Audio),
            ("wav", Category::Audio),
            ("m4a", Category::Audio),
            ("flac", Category::Audio),
            ("aac", Category::Audio),
            ("ogg", Category::Audio),
            ("jpg", Category::Image),
            ("png", Category::Image),
            ("webp", Category::Image),
            ("heic", Category::Image),
            ("bmp", Category::Image),
            ("tiff", Category::Image),
            ("gif", Category::Image),
            ("docx", Category::Document),
            ("pptx", Category::Document),
            ("xlsx", Category::Document),
            ("odt", Category::Document),
            ("rtf", Category::Document),
            ("md", Category::Text),
            ("html", Category::Text),
        ];
        for (id, cat) in expected {
            assert_eq!(normalize_ext(id), Some(id), "{id}");
            assert_eq!(normalize_ext(&id.to_uppercase()), Some(id), "{id} 대문자");
            assert_eq!(category_of(id), Some(cat), "{id}");
            let p = PathBuf::from(format!(r"C:\폴더\발표 녹화.{}", id.to_uppercase()));
            assert_eq!(format_of_path(&p), Some(id), "{id}");
        }
        for (alias, id) in [
            ("jpeg", "jpg"),
            ("JPEG", "jpg"),
            ("heif", "heic"),
            ("tif", "tiff"),
            ("TIF", "tiff"),
            ("htm", "html"),
            ("markdown", "md"),
            (".Jpeg", "jpg"),
        ] {
            assert_eq!(normalize_ext(alias), Some(id), "{alias}");
            assert!(category_of(alias).is_some(), "{alias}");
        }
        assert_eq!(format_of_path(Path::new("a.markdown")), Some("md"));
        assert_eq!(format_of_path(Path::new("a.htm")), Some("html"));
    }

    #[test]
    fn unknown_and_output_only_formats() {
        assert_eq!(normalize_ext("xyz"), None);
        assert_eq!(normalize_ext(""), None);
        assert_eq!(normalize_ext("pdf"), Some("pdf"));
        assert_eq!(normalize_ext("TXT"), Some("txt"));
        assert_eq!(category_of("pdf"), None);
        assert_eq!(format_of_path(Path::new("a.pdf")), None);
        assert_eq!(format_of_path(Path::new("a.xyz")), None);
        assert_eq!(format_of_path(Path::new("확장자없음")), None);
        assert!(outputs_for("xyz").is_err());
        assert!(is_known_output("pdf"));
        assert!(is_known_output("JPEG"));
        assert!(!is_known_output("mov"));
        assert!(!is_known_output("xyz"));
    }

    #[test]
    fn outputs_per_category_without_identity() {
        assert_eq!(
            ids(&outputs_for("mov").expect("mov")),
            ["mp4", "mp3", "gif", "webm"]
        );
        assert_eq!(
            ids(&outputs_for("mp4").expect("mp4")),
            ["mp3", "gif", "webm"]
        );
        assert_eq!(
            ids(&outputs_for("webm").expect("webm")),
            ["mp4", "mp3", "gif"]
        );
        assert_eq!(
            ids(&outputs_for("m4a").expect("m4a")),
            ["mp3", "wav", "flac"]
        );
        assert_eq!(ids(&outputs_for("mp3").expect("mp3")), ["wav", "flac"]);
        assert_eq!(
            ids(&outputs_for("heic").expect("heic")),
            ["jpg", "png", "webp"]
        );
        assert_eq!(ids(&outputs_for("jpeg").expect("jpeg")), ["png", "webp"]);
        assert_eq!(
            ids(&outputs_for("gif").expect("gif")),
            ["jpg", "png", "webp"]
        );
        assert_eq!(ids(&outputs_for("docx").expect("docx")), ["pdf", "txt"]);
        assert_eq!(ids(&outputs_for("xlsx").expect("xlsx")), ["pdf", "txt"]);
        assert_eq!(ids(&outputs_for("pptx").expect("pptx")), ["pdf"]);
        assert_eq!(
            ids(&outputs_for("md").expect("md")),
            ["pdf", "docx", "html"]
        );
        assert_eq!(ids(&outputs_for("htm").expect("htm")), ["pdf", "docx"]);

        let o = outputs_for("mov").expect("mov");
        assert_eq!(o[0].label, "MP4");
        assert_eq!(o[0].description, "영상 (H.264)");
    }

    #[test]
    fn engines_and_support() {
        assert_eq!(engines_for("mov", "mp4"), Some(&[EngineId::Ffmpeg][..]));
        assert_eq!(engines_for("png", "JPEG"), Some(&[EngineId::Vips][..]));
        assert_eq!(engines_for("docx", "pdf"), Some(&[EngineId::Office][..]));
        assert_eq!(engines_for("md", "docx"), Some(&[EngineId::Pandoc][..]));
        assert_eq!(
            engines_for("markdown", "pdf"),
            Some(&[EngineId::Pandoc, EngineId::Office][..])
        );
        assert!(!is_supported("mp4", "mp4"));
        assert!(!is_supported("jpg", "jpeg"));
        assert!(!is_supported("pptx", "txt"));
        assert!(!is_supported("mov", "pdf"));
        assert!(!is_supported("xyz", "mp4"));
        assert!(!is_supported("mov", "xyz"));
        assert!(is_supported("gif", "png"));
    }

    #[test]
    fn common_outputs_intersection() {
        assert_eq!(
            ids(&common_outputs(&["mp4", "mov"]).expect("교집합")),
            ["mp3", "gif", "webm"]
        );
        assert_eq!(
            ids(&common_outputs(&["mov", "mp4"]).expect("교집합")),
            ["mp3", "gif", "webm"]
        );
        assert_eq!(
            ids(&common_outputs(&["mp4", "webm"]).expect("교집합")),
            ["mp3", "gif"]
        );
        assert_eq!(
            ids(&common_outputs(&["mov", "wav"]).expect("교집합")),
            ["mp3"]
        );
        assert!(common_outputs(&["mov", "jpg"]).expect("교집합").is_empty());
        assert!(common_outputs(&[]).expect("빈 입력").is_empty());
        assert!(common_outputs(&["mov", "xyz"]).is_err());
    }

    // ----- 기본 방향 배치 -----

    fn f(id: &str) -> RadialSlot {
        RadialSlot::Format {
            format_id: id.into(),
        }
    }

    #[test]
    fn more_slot_index_convention() {
        assert_eq!(more_slot_index(4), 2);
        assert_eq!(more_slot_index(5), 3);
        assert_eq!(more_slot_index(6), 3);
        assert_eq!(more_slot_index(7), 4);
        assert_eq!(more_slot_index(8), 5);
        // 각도 규약 대조: 칸 i가 135°를 포함하는지 직접 계산
        for n in 4u8..=8 {
            let w = 360.0 / f64::from(n);
            let hit: Vec<usize> = (0..usize::from(n))
                .filter(|&i| {
                    let start = (270.0 + i as f64 * w) % 360.0;
                    let rel = (135.0 - start).rem_euclid(360.0);
                    rel < w
                })
                .collect();
            assert_eq!(hit, vec![more_slot_index(n)], "N={n}");
        }
    }

    #[test]
    fn layout_n4_two_items() {
        let l = default_layout(&["mp4", "mp3"], 4);
        assert_eq!(l.slot_count, 4);
        assert_eq!(
            l.slots,
            vec![f("mp4"), f("mp3"), RadialSlot::Empty, RadialSlot::Empty]
        );
        assert!(l.more.is_empty());
    }

    /// 경계: 항목 N-1개(=3)면 더 보기 없이 0,1,3번 칸, 더 보기 칸(2)은 비운다.
    #[test]
    fn layout_n4_three_items_boundary() {
        let l = default_layout(&["a", "b", "c"], 4);
        assert_eq!(l.slots, vec![f("a"), f("b"), RadialSlot::Empty, f("c")]);
        assert!(l.more.is_empty());
        assert!(!l.slots.contains(&RadialSlot::More));
    }

    /// 경계: 항목 N개(=4)면 형식 3개 + 더 보기(1개). 더 보기 칸에 형식을 넣지 않는다(spec 5장).
    #[test]
    fn layout_n4_four_items_has_more() {
        let l = default_layout(&["mp4", "mp3", "gif", "webm"], 4);
        assert_eq!(
            l.slots,
            vec![f("mp4"), f("mp3"), RadialSlot::More, f("gif")]
        );
        assert_eq!(l.more, vec!["webm"]);
    }

    #[test]
    fn layout_n4_one_item() {
        let l = default_layout(&["a"], 4);
        assert_eq!(
            l.slots,
            vec![
                f("a"),
                RadialSlot::Empty,
                RadialSlot::Empty,
                RadialSlot::Empty
            ]
        );
        assert!(l.more.is_empty());
    }

    #[test]
    fn layout_n4_five_and_six_items() {
        let l = default_layout(&["a", "b", "c", "d", "e"], 4);
        assert_eq!(l.slots, vec![f("a"), f("b"), RadialSlot::More, f("c")]);
        assert_eq!(l.more, vec!["d", "e"]);

        let l = default_layout(&["a", "b", "c", "d", "e", "g"], 4);
        assert_eq!(l.slots, vec![f("a"), f("b"), RadialSlot::More, f("c")]);
        assert_eq!(l.more, vec!["d", "e", "g"]);
    }

    #[test]
    fn layout_empty_and_clamped() {
        let l = default_layout(&[], 4);
        assert_eq!(l.slots, vec![RadialSlot::Empty; 4]);
        let l = default_layout(&["a"], 2);
        assert_eq!(l.slot_count, 4);
        assert_eq!(l.slots.len(), 4);
        let l = default_layout(&["a"], 20);
        assert_eq!(l.slot_count, 8);
        assert_eq!(l.slots.len(), 8);
    }

    #[test]
    fn layout_n8() {
        // 경계: 항목 N-1개(=7)면 더 보기 칸 5만 비운다
        let items = ["1", "2", "3", "4", "5", "6", "7"];
        let l = default_layout(&items, 8);
        assert_eq!(
            l.slots,
            vec![
                f("1"),
                f("2"),
                f("3"),
                f("4"),
                f("5"),
                RadialSlot::Empty,
                f("6"),
                f("7")
            ]
        );
        assert!(l.more.is_empty());

        // 경계: 항목 N개(=8)면 7개 + 더 보기(1개)
        let items = ["1", "2", "3", "4", "5", "6", "7", "8"];
        let l = default_layout(&items, 8);
        assert_eq!(
            l.slots,
            vec![
                f("1"),
                f("2"),
                f("3"),
                f("4"),
                f("5"),
                RadialSlot::More,
                f("6"),
                f("7")
            ]
        );
        assert_eq!(l.more, vec!["8"]);

        let items = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
        let l = default_layout(&items, 8);
        assert_eq!(
            l.slots,
            vec![
                f("1"),
                f("2"),
                f("3"),
                f("4"),
                f("5"),
                RadialSlot::More,
                f("6"),
                f("7")
            ]
        );
        assert_eq!(l.more, vec!["8", "9", "10"]);

        let l = default_layout(&["1", "2", "3"], 8);
        assert_eq!(l.slots[0..3], [f("1"), f("2"), f("3")]);
        assert!(l.slots[3..].iter().all(|s| *s == RadialSlot::Empty));
    }

    // ----- query_formats -----

    fn all_engines() -> EngineSet {
        let mut paths = BTreeMap::new();
        for id in EngineId::ALL {
            paths.insert(id, PathBuf::from(format!(r"C:\e\{id}.exe")));
        }
        EngineSet::from_parts(paths, None, None)
    }

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn query_single_file() {
        let r = query_formats(
            &[pb(r"C:\영상\발표 녹화.mov")],
            Some(Path::new(r"C:\영상\발표 녹화.mov")),
            &all_engines(),
        )
        .expect("질의");
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].format, "mov");
        assert_eq!(r.files[0].category, Category::Video);
        let opt_ids: Vec<&str> = r.options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(opt_ids, ["mp4", "mp3", "gif", "webm"]);
        assert!(r
            .options
            .iter()
            .all(|o| o.available && o.unavailable.is_none()));
        assert_eq!(
            r.layout.slots,
            vec![f("mp4"), f("mp3"), RadialSlot::More, f("gif")]
        );
        assert_eq!(r.layout.more, vec!["webm"]);
    }

    #[test]
    fn query_primary_filters_other_categories() {
        let files = [
            pb(r"C:\a\발표 녹화.mov"),
            pb(r"C:\a\b.mp4"),
            pb(r"C:\a\사진.jpg"),
            pb(r"C:\a\메모.xyz"),
            pb(r"C:\a\노래.wav"),
        ];
        let r =
            query_formats(&files, Some(Path::new(r"C:\a\b.mp4")), &all_engines()).expect("질의");
        let kept: Vec<&str> = r.files.iter().map(|f| f.format.as_str()).collect();
        assert_eq!(kept, ["mov", "mp4"]);
        assert_eq!(
            r.skipped,
            vec![
                SkippedFile {
                    path: files[2].clone(),
                    reason: SkipReason::DifferentCategory
                },
                SkippedFile {
                    path: files[3].clone(),
                    reason: SkipReason::UnsupportedFormat
                },
                SkippedFile {
                    path: files[4].clone(),
                    reason: SkipReason::DifferentCategory
                },
            ]
        );
        let opt_ids: Vec<&str> = r.options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(opt_ids, ["mp3", "gif", "webm"]);
        assert_eq!(
            r.layout.slots,
            vec![f("mp3"), f("gif"), RadialSlot::Empty, f("webm")]
        );
    }

    #[test]
    fn query_without_primary_uses_intersection() {
        let files = [pb("a.mov"), pb("b.jpg")];
        let err = query_formats(&files, None, &all_engines()).expect_err("교집합 없음");
        assert_eq!(err.code(), crate::error::ErrorCode::UnsupportedFormat);
    }

    #[test]
    fn query_errors() {
        let err = query_formats(&[pb("a.xyz")], None, &all_engines()).expect_err("미지원");
        assert_eq!(err.to_string(), "지원하지 않는 형식입니다: xyz → ?");
        let err = query_formats(&[], None, &all_engines()).expect_err("빈 목록");
        assert_eq!(err.code(), crate::error::ErrorCode::UnsupportedFormat);
        // primary가 지원하지 않는 형식이면 분류 걸러내기를 하지 않는다
        let r = query_formats(
            &[pb("a.mov"), pb("b.xyz")],
            Some(Path::new("b.xyz")),
            &all_engines(),
        )
        .expect("질의");
        assert_eq!(r.files.len(), 1);
    }

    #[test]
    fn query_availability() {
        let none = EngineSet::default();
        let r = query_formats(&[pb("문서.md")], None, &none).expect("질의");
        let pdf = &r.options[0];
        assert_eq!(pdf.id, "pdf");
        assert!(!pdf.available);
        let u = pdf.unavailable.as_ref().expect("사용 불가");
        assert_eq!(u.code, UnavailableCode::EngineMissing);
        assert_eq!(u.engine, Some(EngineId::Pandoc));
        assert_eq!(u.message, "Pandoc이 설치되지 않았습니다");

        // pandoc만 있음 → pdf는 office 없음
        let mut paths = BTreeMap::new();
        paths.insert(EngineId::Pandoc, pb("pandoc.exe"));
        let set = EngineSet::from_parts(paths, None, None);
        let r = query_formats(&[pb("문서.md")], None, &set).expect("질의");
        let u = r.options[0].unavailable.as_ref().expect("office 없음");
        assert_eq!(u.engine, Some(EngineId::Office));
        assert_eq!(u.message, "LibreOffice가 설치되지 않았습니다");
        assert!(r.options[1].available);

        // ffmpeg 인코더
        let mut paths = BTreeMap::new();
        paths.insert(EngineId::Ffmpeg, pb("ffmpeg.exe"));
        let caps = FfmpegCaps {
            h264: None,
            vp9: true,
            opus: false,
            mp3lame: true,
        };
        let set = EngineSet::from_parts(paths.clone(), Some(caps), None);
        let r = query_formats(&[pb("a.mov")], None, &set).expect("질의");
        let by_id = |id: &str| r.options.iter().find(|o| o.id == id).expect(id).clone();
        let mp4 = by_id("mp4");
        assert_eq!(
            mp4.unavailable.as_ref().map(|u| u.code),
            Some(UnavailableCode::CodecMissing)
        );
        assert_eq!(
            mp4.unavailable.as_ref().map(|u| u.message.as_str()),
            Some("FFmpeg에 H.264 인코더(h264_mf, libopenh264)가 없습니다")
        );
        assert!(!by_id("webm").available);
        assert!(by_id("mp3").available);
        assert!(by_id("gif").available);

        let caps = FfmpegCaps {
            h264: Some(H264Encoder::OpenH264),
            vp9: true,
            opus: true,
            mp3lame: false,
        };
        let set = EngineSet::from_parts(paths, Some(caps), None);
        let r = query_formats(&[pb("a.wav")], None, &set).expect("질의");
        assert!(!r.options[0].available, "mp3lame 없음");
        assert!(r.options[1].available);

        // heic
        let mut paths = BTreeMap::new();
        paths.insert(EngineId::Vips, pb("vips.exe"));
        let set = EngineSet::from_parts(paths.clone(), None, Some(false));
        let r = query_formats(&[pb("사진.HEIC")], None, &set).expect("질의");
        assert!(r.options.iter().all(|o| !o.available));
        assert_eq!(
            r.options[0]
                .unavailable
                .as_ref()
                .map(|u| u.message.as_str()),
            Some("이 libvips 빌드는 HEIC를 읽지 못합니다")
        );
        let r = query_formats(&[pb("사진.png")], None, &set).expect("질의");
        assert!(r.options.iter().all(|o| o.available));
        let set = EngineSet::from_parts(paths, None, None);
        let r = query_formats(&[pb("사진.heif")], None, &set).expect("질의");
        assert!(r.options.iter().all(|o| o.available), "probe 전이면 가능");
    }
}
