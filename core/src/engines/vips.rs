//! libvips (`vips.exe`) 어댑터.
//!
//! 저장 옵션은 출력 파일 이름 뒤 `[...]`로 붙인다(vips 문법). 셸을 거치지 않으므로
//! 경로 전체와 옵션이 인자 하나로 넘어간다.

use std::ffi::OsString;
use std::path::Path;

use super::{CommandSpec, ProgressStream};
use crate::error::CoreError;
use crate::presets::{self, PresetId};

pub const EXE: &str = "vips.exe";

/// `vips -l` 출력에 `heifload` 연산이 있으면 HEIC/HEIF를 읽을 수 있다.
/// 예: `VipsForeignLoadHeifFile (heifload), load a HEIF image (.heic, .heif, .avif), priority=0, ...`
pub fn parse_heic_support(list_stdout: &str) -> bool {
    list_stdout.lines().any(|line| {
        line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|tok| tok == "heifload")
    })
}

/// 크기 제한 없음: `vips autorot <in> "<out>[Q=..,strip]"`
/// 크기 제한 있음: `vips thumbnail <in> "<out>[..]" <max> --height <max> --size down`
/// png: `[compression=N,strip]`, jpg·webp: `[Q=N,strip]`, progress = None
///
/// 크기 제한이 없을 때 `copy` 대신 `autorot`을 쓴다. `strip`이 EXIF 방향 태그를 지우므로
/// 그대로 복사하면 세로 사진이 눕는다. `autorot`은 방향대로 픽셀을 돌리고 태그를 지운다
/// (방향 태그가 없으면 copy와 같다). `thumbnail`은 기본으로 방향을 적용한다.
pub fn build_command(
    vips: &Path,
    input: &Path,
    output: &Path,
    to: &str,
    preset: PresetId,
) -> Result<CommandSpec, CoreError> {
    let params = presets::image(preset);
    let options = match to {
        "jpg" | "webp" => format!("Q={},strip", params.quality),
        "png" => format!("compression={},strip", params.png_compression),
        other => {
            return Err(CoreError::UnsupportedFormat {
                input: input
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_else(|| "?".to_string()),
                to: Some(other.to_string()),
            })
        }
    };
    let mut out_arg = output.as_os_str().to_owned();
    out_arg.push(format!("[{options}]"));

    let mut args: Vec<OsString> = Vec::with_capacity(8);
    match params.max_dim {
        None => {
            args.push("autorot".into());
            args.push(input.as_os_str().to_owned());
            args.push(out_arg);
        }
        Some(max) => {
            args.push("thumbnail".into());
            args.push(input.as_os_str().to_owned());
            args.push(out_arg);
            args.push(max.to_string().into());
            args.push("--height".into());
            args.push(max.to_string().into());
            args.push("--size".into());
            args.push("down".into());
        }
    }

    Ok(CommandSpec {
        program: vips.to_path_buf(),
        args,
        cwd: None,
        progress: ProgressStream::None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn strs(spec: &CommandSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn heic_support_detection() {
        let with = "\
  VipsForeignLoadJpegFile (jpegload), load jpeg from file (.jpg, .jpeg, .jpe, .jfif), priority=50, is_a, get_flags, get_flags_filename, header, load
  VipsForeignLoadHeifFile (heifload), load a HEIF image (.heic, .heif, .avif), priority=0, is_a, get_flags, header, load
  VipsForeignSaveHeifFile (heifsave), save image in HEIF format (.heic, .heif), priority=0, rgb
";
        assert!(parse_heic_support(with));
        let without = "\
  VipsForeignLoadJpegFile (jpegload), load jpeg from file (.jpg, .jpeg), priority=50
  VipsForeignLoadPngFile (pngload), load png from file (.png), priority=200
";
        assert!(!parse_heic_support(without));
        assert!(!parse_heic_support(""));
        // 비슷한 이름은 인정하지 않는다
        assert!(!parse_heic_support(
            "VipsForeignLoadX (heifload_buffer_only)"
        ));
        assert!(parse_heic_support(
            "heifload_source\nVipsForeignLoadHeifFile (heifload)"
        ));
    }

    #[test]
    fn thumbnail_branch_for_web_jpg() {
        let exe = PathBuf::from(r"C:\엔진\vips\bin\vips.exe");
        let input = PathBuf::from(r"C:\사진 모음\여름 휴가.heic");
        let out = PathBuf::from(r"C:\사진 모음\.여름 휴가.4.convtool-tmp.jpg");
        let spec = build_command(&exe, &input, &out, "jpg", PresetId::Web).expect("jpg");
        assert_eq!(spec.program, exe);
        assert_eq!(spec.progress, ProgressStream::None);
        assert_eq!(
            strs(&spec),
            vec![
                "thumbnail",
                r"C:\사진 모음\여름 휴가.heic",
                r"C:\사진 모음\.여름 휴가.4.convtool-tmp.jpg[Q=82,strip]",
                "2560",
                "--height",
                "2560",
                "--size",
                "down",
            ]
        );
    }

    #[test]
    fn autorot_branch_for_original() {
        let exe = PathBuf::from("vips.exe");
        let input = PathBuf::from(r"C:\사진 모음\a b.png");
        let out = PathBuf::from(r"C:\사진 모음\.a b.1.convtool-tmp.webp");
        let spec = build_command(&exe, &input, &out, "webp", PresetId::Original).expect("webp");
        assert_eq!(
            strs(&spec),
            vec![
                "autorot",
                r"C:\사진 모음\a b.png",
                r"C:\사진 모음\.a b.1.convtool-tmp.webp[Q=95,strip]",
            ]
        );
    }

    #[test]
    fn png_compression_and_small() {
        let exe = PathBuf::from("vips.exe");
        let input = PathBuf::from(r"C:\x\사진.jpg");
        let out = PathBuf::from(r"C:\x\.사진.2.convtool-tmp.png");
        let small = build_command(&exe, &input, &out, "png", PresetId::Small).expect("png");
        let a = strs(&small);
        assert_eq!(a[0], "thumbnail");
        assert_eq!(a[2], r"C:\x\.사진.2.convtool-tmp.png[compression=9,strip]");
        assert_eq!(a[3], "1280");
        assert_eq!(a[5], "1280");
        let orig = build_command(&exe, &input, &out, "png", PresetId::Original).expect("png");
        assert_eq!(
            strs(&orig)[2],
            r"C:\x\.사진.2.convtool-tmp.png[compression=6,strip]"
        );
    }

    #[test]
    fn unsupported_target() {
        let err = build_command(
            Path::new("vips.exe"),
            Path::new(r"C:\x\a.PNG"),
            Path::new(r"C:\x\a.pdf"),
            "pdf",
            PresetId::Web,
        )
        .expect_err("pdf 불가");
        match err {
            CoreError::UnsupportedFormat { input, to } => {
                assert_eq!(input, "png");
                assert_eq!(to.as_deref(), Some("pdf"));
            }
            other => panic!("예상하지 않은 오류: {other:?}"),
        }
    }
}
