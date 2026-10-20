//! FFmpeg (LGPL 빌드) 어댑터.
//!
//! 명령 인자 생성과 출력 파싱만 담는 순수 함수 모음이다. 실행은 `runner.rs`가 한다.
//!
//! - LGPL 빌드에는 `libx264`가 없으므로 H.264는 `h264_mf`(Media Foundation)를 먼저,
//!   없으면 `libopenh264`를 쓴다.
//! - 진행률은 `-progress pipe:1 -nostats`로 표준 출력의 `out_time_us=`를 읽는다.

use std::ffi::OsString;
use std::path::Path;

use super::{CommandSpec, EngineId, ProgressStream, ProgressUpdate};
use crate::error::CoreError;
use crate::presets::{self, PresetId};

pub const EXE: &str = "ffmpeg.exe";
pub const PROBE_EXE: &str = "ffprobe.exe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Encoder {
    /// h264_mf
    MediaFoundation,
    /// libopenh264
    OpenH264,
}

impl H264Encoder {
    /// `-c:v`에 넘기는 인코더 이름
    pub fn codec_name(self) -> &'static str {
        match self {
            H264Encoder::MediaFoundation => "h264_mf",
            H264Encoder::OpenH264 => "libopenh264",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FfmpegCaps {
    pub h264: Option<H264Encoder>,
    pub vp9: bool,
    pub opus: bool,
    pub mp3lame: bool,
}

/// `ffmpeg -hide_banner -encoders` 표준 출력을 읽는다. h264_mf를 libopenh264보다 우선한다.
///
/// 인코더 줄 형식: ` V....D h264_mf              H264 via MediaFoundation (codec h264)`.
/// 첫 토큰이 6글자 플래그이고 두 번째 토큰이 인코더 이름이다. 머리말과 `------` 줄은
/// 플래그 모양이 아니므로 건너뛴다.
pub fn parse_encoders(encoders_stdout: &str) -> FfmpegCaps {
    let mut has_mf = false;
    let mut has_openh264 = false;
    let mut caps = FfmpegCaps::default();
    for line in encoders_stdout.lines() {
        let mut tokens = line.split_whitespace();
        let (Some(flags), Some(name)) = (tokens.next(), tokens.next()) else {
            continue;
        };
        if !is_encoder_flags(flags) {
            continue;
        }
        match name {
            "h264_mf" => has_mf = true,
            "libopenh264" => has_openh264 = true,
            "libvpx-vp9" => caps.vp9 = true,
            "libopus" => caps.opus = true,
            "libmp3lame" => caps.mp3lame = true,
            _ => {}
        }
    }
    caps.h264 = if has_mf {
        Some(H264Encoder::MediaFoundation)
    } else if has_openh264 {
        Some(H264Encoder::OpenH264)
    } else {
        None
    };
    caps
}

/// 플래그 칸: 6글자, 첫 글자는 V/A/S, 나머지는 영문자 또는 '.'
fn is_encoder_flags(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() == 6
        && matches!(bytes[0], b'V' | b'A' | b'S')
        && bytes[1..]
            .iter()
            .all(|b| *b == b'.' || b.is_ascii_alphabetic())
}

/// `ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 <in>`
pub fn probe_duration_command(ffprobe: &Path, input: &Path) -> CommandSpec {
    let args: Vec<OsString> = vec![
        "-v".into(),
        "error".into(),
        "-show_entries".into(),
        "format=duration".into(),
        "-of".into(),
        "default=noprint_wrappers=1:nokey=1".into(),
        input.as_os_str().to_owned(),
    ];
    CommandSpec {
        program: ffprobe.to_path_buf(),
        args,
        cwd: None,
        progress: ProgressStream::None,
    }
}

/// ffprobe 출력에서 길이(초)를 읽는다. `12.345000`과 `duration=12.345000` 둘 다 받는다.
/// `N/A`, 0 이하, 숫자가 아닌 값은 None.
pub fn parse_duration(probe_stdout: &str) -> Option<f64> {
    probe_stdout.lines().find_map(|line| {
        let line = line.trim();
        let value = line.strip_prefix("duration=").unwrap_or(line);
        value
            .parse::<f64>()
            .ok()
            .filter(|d| d.is_finite() && *d > 0.0)
    })
}

/// 공통: -hide_banner -nostdin -nostats -y -progress pipe:1 -i <in> … <out>, progress = Stdout
/// mp4: -c:v h264_mf|libopenh264 [-b:v Nk] -pix_fmt yuv420p -vf <짝수 크기 scale> -c:a aac -b:a Nk -movflags +faststart
/// webm: -c:v libvpx-vp9 -b:v 0 -crf N -row-mt 1 -pix_fmt yuv420p -vf <scale> -c:a libopus -b:a Nk
/// gif: -filter_complex "[0:v]fps=F,scale=W:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse" -an -loop 0
/// mp3: -vn -c:a libmp3lame (-q:a | -b:a), wav: -vn -c:a pcm_s16le, flac: -vn -c:a flac
/// 인코더가 없으면 Err(CodecMissing)
pub fn build_command(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    to: &str,
    preset: PresetId,
    caps: &FfmpegCaps,
) -> Result<CommandSpec, CoreError> {
    let video = presets::video(preset);
    let mut args: Vec<OsString> = Vec::with_capacity(32);
    let mut push = |s: &str| args.push(OsString::from(s));
    for a in [
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-y",
        "-progress",
        "pipe:1",
    ] {
        push(a);
    }
    push("-i");
    args.push(input.as_os_str().to_owned());

    let mut push = |s: &str| args.push(OsString::from(s));
    match to {
        "mp4" => {
            let enc = caps.h264.ok_or_else(|| CoreError::CodecMissing {
                engine: EngineId::Ffmpeg,
                codec: "H.264(h264_mf, libopenh264)".into(),
            })?;
            push("-c:v");
            push(enc.codec_name());
            if let Some(kbps) = video.video_kbps {
                push("-b:v");
                push(&format!("{kbps}k"));
            }
            push("-pix_fmt");
            push("yuv420p");
            push("-vf");
            push(&even_scale_filter(video.max_height));
            push("-c:a");
            push("aac");
            push("-b:a");
            push(&format!("{}k", video.audio_kbps));
            push("-movflags");
            push("+faststart");
        }
        "webm" => {
            if !caps.vp9 {
                return Err(codec_missing("VP9(libvpx-vp9)"));
            }
            if !caps.opus {
                return Err(codec_missing("Opus(libopus)"));
            }
            push("-c:v");
            push("libvpx-vp9");
            push("-b:v");
            push("0");
            push("-crf");
            push(&video.vp9_crf.to_string());
            push("-row-mt");
            push("1");
            push("-pix_fmt");
            push("yuv420p");
            push("-vf");
            push(&even_scale_filter(video.max_height));
            push("-c:a");
            push("libopus");
            push("-b:a");
            push(&format!("{}k", video.audio_kbps));
        }
        "gif" => {
            push("-filter_complex");
            push(&gif_filter(video.gif_fps, video.gif_width));
            push("-an");
            push("-loop");
            push("0");
        }
        "mp3" => {
            if !caps.mp3lame {
                return Err(codec_missing("MP3(libmp3lame)"));
            }
            let audio = presets::audio(preset);
            push("-vn");
            push("-c:a");
            push("libmp3lame");
            match (audio.mp3_vbr_q, audio.mp3_kbps) {
                (Some(q), _) => {
                    push("-q:a");
                    push(&q.to_string());
                }
                (None, Some(kbps)) => {
                    push("-b:a");
                    push(&format!("{kbps}k"));
                }
                (None, None) => {
                    push("-q:a");
                    push("2");
                }
            }
        }
        "wav" => {
            push("-vn");
            push("-c:a");
            push("pcm_s16le");
        }
        "flac" => {
            push("-vn");
            push("-c:a");
            push("flac");
        }
        other => {
            return Err(CoreError::UnsupportedFormat {
                input: input_ext(input),
                to: Some(other.to_string()),
            })
        }
    }
    args.push(output.as_os_str().to_owned());

    Ok(CommandSpec {
        program: ffmpeg.to_path_buf(),
        args,
        cwd: None,
        progress: ProgressStream::Stdout,
    })
}

fn codec_missing(codec: &str) -> CoreError {
    CoreError::CodecMissing {
        engine: EngineId::Ffmpeg,
        codec: codec.into(),
    }
}

/// H.264 yuv420p는 가로·세로가 짝수여야 한다. 높이 상한이 있으면 원본보다 키우지 않는다.
/// 작은따옴표는 FFmpeg 필터 문법의 인용이다(쉼표를 필터 구분자로 읽지 않게). 셸을 거치지 않는다.
fn even_scale_filter(max_height: Option<u32>) -> String {
    match max_height {
        Some(h) => format!("scale=-2:'min({h},trunc(ih/2)*2)'"),
        None => "scale=trunc(iw/2)*2:trunc(ih/2)*2".to_string(),
    }
}

fn gif_filter(fps: u32, width: u32) -> String {
    format!(
        "[0:v]fps={fps},scale={width}:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse"
    )
}

fn input_ext(input: &Path) -> String {
    input
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "?".to_string())
}

/// "out_time_us=…" → 길이가 있으면 Fraction(out/duration, 0..=1로 자름), 없거나 값이 N/A면 Unknown.
/// "progress=end" → End. 그 밖 → None.
///
/// `out_time_ms=`도 FFmpeg 버그로 마이크로초를 담지만, 같은 값을 두 번 보내지 않도록 읽지 않는다.
pub fn parse_progress(line: &str, duration_secs: Option<f64>) -> Option<ProgressUpdate> {
    let line = line.trim();
    if let Some(value) = line.strip_prefix("out_time_us=") {
        let Ok(us) = value.trim().parse::<i64>() else {
            return Some(ProgressUpdate::Unknown);
        };
        return Some(match duration_secs {
            Some(d) if d.is_finite() && d > 0.0 => {
                let secs = us.max(0) as f64 / 1_000_000.0;
                ProgressUpdate::Fraction((secs / d).clamp(0.0, 1.0))
            }
            _ => ProgressUpdate::Unknown,
        });
    }
    if line == "progress=end" {
        return Some(ProgressUpdate::End);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ENCODERS_SAMPLE: &str = "\
Encoders:
 V..... = Video
 A..... = Audio
 S..... = Subtitle
 .F.... = Frame-level multithreading
 ..S... = Slice-level multithreading
 ...X.. = Codec is experimental
 ....B. = Supports draw_horiz_band
 .....D = Supports direct rendering method 1
 ------
 V....D libopenh264          OpenH264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
 V....D h264_mf              H264 via MediaFoundation (codec h264)
 V....D libvpx-vp9           libvpx VP9 (codec vp9)
 A....D aac                  AAC (Advanced Audio Coding)
 A....D libmp3lame           libmp3lame MP3 (MPEG audio layer 3) (codec mp3)
 A....D libopus              libopus Opus (codec opus)
 A....D pcm_s16le            PCM signed 16-bit little-endian
";

    fn all_caps() -> FfmpegCaps {
        FfmpegCaps {
            h264: Some(H264Encoder::MediaFoundation),
            vp9: true,
            opus: true,
            mp3lame: true,
        }
    }

    fn strs(spec: &CommandSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn paths() -> (PathBuf, PathBuf, PathBuf) {
        (
            PathBuf::from(r"C:\엔진\ffmpeg\ffmpeg.exe"),
            PathBuf::from(r"C:\영상 폴더\발표 녹화.mov"),
            PathBuf::from(r"C:\영상 폴더\.발표 녹화.7.convtool-tmp.mp4"),
        )
    }

    #[test]
    fn parse_encoders_prefers_media_foundation() {
        let caps = parse_encoders(ENCODERS_SAMPLE);
        assert_eq!(caps, all_caps());
    }

    #[test]
    fn parse_encoders_falls_back_to_openh264() {
        let s = ENCODERS_SAMPLE.replace(" V....D h264_mf ", " V....D other_mf ");
        let caps = parse_encoders(&s);
        assert_eq!(caps.h264, Some(H264Encoder::OpenH264));
    }

    #[test]
    fn parse_encoders_ignores_legend_and_empty() {
        // 머리말의 "V..... = Video" 줄은 이름 칸이 "="라 어떤 인코더로도 읽히지 않는다.
        let caps = parse_encoders("Encoders:\n V..... = Video\n ------\n");
        assert_eq!(caps, FfmpegCaps::default());
        assert_eq!(parse_encoders(""), FfmpegCaps::default());
        // 설명 칸에 이름이 들어 있어도 인코더 이름 칸만 본다
        let caps = parse_encoders(" A....D aac   AAC like libmp3lame\n");
        assert!(!caps.mp3lame);
    }

    #[test]
    fn probe_command_keeps_path_as_one_arg() {
        let (_, input, _) = paths();
        let spec = probe_duration_command(Path::new(r"C:\엔진\ffprobe.exe"), &input);
        assert_eq!(spec.program, PathBuf::from(r"C:\엔진\ffprobe.exe"));
        assert_eq!(
            strs(&spec),
            vec![
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                r"C:\영상 폴더\발표 녹화.mov",
            ]
        );
        assert_eq!(spec.progress, ProgressStream::None);
    }

    #[test]
    fn parse_duration_examples() {
        assert_eq!(parse_duration("12.345000\r\n"), Some(12.345));
        assert_eq!(parse_duration("duration=3.5\n"), Some(3.5));
        assert_eq!(parse_duration("N/A\n"), None);
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("0.000000\n"), None);
        assert_eq!(parse_duration("garbage\n60.0\n"), Some(60.0));
    }

    #[test]
    fn parse_progress_examples() {
        assert_eq!(
            parse_progress("out_time_us=5000000", Some(10.0)),
            Some(ProgressUpdate::Fraction(0.5))
        );
        assert_eq!(
            parse_progress("out_time_us=15000000\r", Some(10.0)),
            Some(ProgressUpdate::Fraction(1.0))
        );
        assert_eq!(
            parse_progress("out_time_us=-23220", Some(10.0)),
            Some(ProgressUpdate::Fraction(0.0))
        );
        assert_eq!(
            parse_progress("out_time_us=5000000", None),
            Some(ProgressUpdate::Unknown)
        );
        assert_eq!(
            parse_progress("out_time_us=5000000", Some(0.0)),
            Some(ProgressUpdate::Unknown)
        );
        assert_eq!(
            parse_progress("out_time_us=N/A", Some(10.0)),
            Some(ProgressUpdate::Unknown)
        );
        assert_eq!(
            parse_progress("progress=end", Some(10.0)),
            Some(ProgressUpdate::End)
        );
        assert_eq!(parse_progress("progress=continue", Some(10.0)), None);
        assert_eq!(parse_progress("out_time_ms=5000000", Some(10.0)), None);
        assert_eq!(parse_progress("frame=120", Some(10.0)), None);
    }

    #[test]
    fn mp4_args_snapshot_with_korean_spaces() {
        let (exe, input, output) = paths();
        let spec =
            build_command(&exe, &input, &output, "mp4", PresetId::Web, &all_caps()).expect("mp4");
        assert_eq!(spec.program, exe);
        assert_eq!(spec.progress, ProgressStream::Stdout);
        assert_eq!(spec.cwd, None);
        assert_eq!(
            strs(&spec),
            vec![
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-y",
                "-progress",
                "pipe:1",
                "-i",
                r"C:\영상 폴더\발표 녹화.mov",
                "-c:v",
                "h264_mf",
                "-b:v",
                "5000k",
                "-pix_fmt",
                "yuv420p",
                "-vf",
                "scale=-2:'min(1080,trunc(ih/2)*2)'",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-movflags",
                "+faststart",
                r"C:\영상 폴더\.발표 녹화.7.convtool-tmp.mp4",
            ]
        );
    }

    #[test]
    fn mp4_original_keeps_size_but_even() {
        let (exe, input, output) = paths();
        let spec = build_command(
            &exe,
            &input,
            &output,
            "mp4",
            PresetId::Original,
            &all_caps(),
        )
        .expect("mp4");
        let a = strs(&spec);
        assert!(a.contains(&"scale=trunc(iw/2)*2:trunc(ih/2)*2".to_string()));
        assert!(a.contains(&"12000k".to_string()));
        assert!(a.contains(&"192k".to_string()));
    }

    #[test]
    fn mp4_encoder_selection() {
        let (exe, input, output) = paths();
        let mut caps = all_caps();
        caps.h264 = Some(H264Encoder::OpenH264);
        let spec = build_command(&exe, &input, &output, "mp4", PresetId::Small, &caps).expect("ok");
        let a = strs(&spec);
        let i = a.iter().position(|s| s == "-c:v").expect("-c:v");
        assert_eq!(a[i + 1], "libopenh264");
        assert!(a.contains(&"scale=-2:'min(720,trunc(ih/2)*2)'".to_string()));

        caps.h264 = None;
        let err = build_command(&exe, &input, &output, "mp4", PresetId::Web, &caps)
            .expect_err("인코더 없음");
        match err {
            CoreError::CodecMissing { engine, codec } => {
                assert_eq!(engine, EngineId::Ffmpeg);
                assert!(codec.contains("h264_mf") && codec.contains("libopenh264"));
            }
            other => panic!("예상하지 않은 오류: {other:?}"),
        }
    }

    #[test]
    fn webm_args_and_missing_codecs() {
        let (exe, input, _) = paths();
        let out = PathBuf::from(r"C:\영상 폴더\.발표 녹화.7.convtool-tmp.webm");
        let spec =
            build_command(&exe, &input, &out, "webm", PresetId::Web, &all_caps()).expect("webm");
        let a = strs(&spec);
        assert_eq!(
            &a[8..],
            &[
                "-c:v",
                "libvpx-vp9",
                "-b:v",
                "0",
                "-crf",
                "32",
                "-row-mt",
                "1",
                "-pix_fmt",
                "yuv420p",
                "-vf",
                "scale=-2:'min(1080,trunc(ih/2)*2)'",
                "-c:a",
                "libopus",
                "-b:a",
                "128k",
                r"C:\영상 폴더\.발표 녹화.7.convtool-tmp.webm",
            ]
        );

        let mut caps = all_caps();
        caps.vp9 = false;
        assert!(matches!(
            build_command(&exe, &input, &out, "webm", PresetId::Web, &caps),
            Err(CoreError::CodecMissing { .. })
        ));
        let mut caps = all_caps();
        caps.opus = false;
        assert!(matches!(
            build_command(&exe, &input, &out, "webm", PresetId::Web, &caps),
            Err(CoreError::CodecMissing { .. })
        ));
    }

    #[test]
    fn gif_filter_string() {
        let (exe, input, _) = paths();
        let out = PathBuf::from(r"C:\영상 폴더\.발표 녹화.7.convtool-tmp.gif");
        // gif는 외부 인코더가 필요 없다: caps가 비어 있어도 된다
        let spec = build_command(
            &exe,
            &input,
            &out,
            "gif",
            PresetId::Web,
            &FfmpegCaps::default(),
        )
        .expect("gif");
        let a = strs(&spec);
        assert_eq!(
            &a[8..],
            &[
                "-filter_complex",
                "[0:v]fps=12,scale=480:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse",
                "-an",
                "-loop",
                "0",
                r"C:\영상 폴더\.발표 녹화.7.convtool-tmp.gif",
            ]
        );
        let small = build_command(
            &exe,
            &input,
            &out,
            "gif",
            PresetId::Small,
            &FfmpegCaps::default(),
        )
        .expect("gif");
        assert!(strs(&small)[9].starts_with("[0:v]fps=10,scale=320:-1:"));
    }

    #[test]
    fn audio_outputs() {
        let exe = PathBuf::from("ffmpeg.exe");
        let input = PathBuf::from(r"C:\음악\노래 1.flac");
        let out = PathBuf::from(r"C:\음악\.노래 1.3.convtool-tmp.mp3");

        let web =
            build_command(&exe, &input, &out, "mp3", PresetId::Web, &all_caps()).expect("mp3");
        assert_eq!(
            &strs(&web)[8..],
            &[
                "-vn",
                "-c:a",
                "libmp3lame",
                "-q:a",
                "2",
                out.to_str().expect("utf8")
            ]
        );
        let orig =
            build_command(&exe, &input, &out, "mp3", PresetId::Original, &all_caps()).expect("mp3");
        assert_eq!(&strs(&orig)[11..13], &["-b:a", "320k"]);
        let small =
            build_command(&exe, &input, &out, "mp3", PresetId::Small, &all_caps()).expect("mp3");
        assert_eq!(&strs(&small)[11..13], &["-q:a", "6"]);

        let mut caps = all_caps();
        caps.mp3lame = false;
        assert!(matches!(
            build_command(&exe, &input, &out, "mp3", PresetId::Web, &caps),
            Err(CoreError::CodecMissing { .. })
        ));

        let wav = build_command(
            &exe,
            &input,
            &out,
            "wav",
            PresetId::Web,
            &FfmpegCaps::default(),
        )
        .expect("wav");
        assert_eq!(&strs(&wav)[8..11], &["-vn", "-c:a", "pcm_s16le"]);
        let flac = build_command(
            &exe,
            &input,
            &out,
            "flac",
            PresetId::Web,
            &FfmpegCaps::default(),
        )
        .expect("flac");
        assert_eq!(&strs(&flac)[8..11], &["-vn", "-c:a", "flac"]);
    }

    #[test]
    fn unsupported_target() {
        let (exe, input, output) = paths();
        let err = build_command(&exe, &input, &output, "pdf", PresetId::Web, &all_caps())
            .expect_err("pdf 불가");
        match err {
            CoreError::UnsupportedFormat { input, to } => {
                assert_eq!(input, "mov");
                assert_eq!(to.as_deref(), Some("pdf"));
            }
            other => panic!("예상하지 않은 오류: {other:?}"),
        }
    }
}
