//! LibreOffice (`soffice --headless`) 어댑터.
//!
//! - 출력 이름을 지정할 수 없으므로 `--outdir <작업 폴더>`에 받고, 확정은 job.rs가 한다.
//! - 사용자가 LibreOffice를 열어 두었어도 실패하지 않도록 작업마다 별도 프로필
//!   (`-env:UserInstallation=file:///…`)을 쓴다.
//! - 느리므로 동시 실행은 [`MAX_PARALLEL`]개로 제한한다(큐가 지킨다).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{CommandSpec, ProgressStream};
use crate::error::CoreError;

pub const EXE: &str = "soffice.exe";
pub const MAX_PARALLEL: usize = 1;

/// 시스템에 설치된 LibreOffice 후보 경로(앞이 우선, 중복 제거).
/// `%ProgramFiles%`, `%ProgramW6432%`(32비트 프로세스에서 64비트 폴더), `%ProgramFiles(x86)%`
/// 아래 `LibreOffice\program\soffice.exe`. 존재 여부는 확인하지 않는다.
pub fn system_install_candidates(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        let Some(base) = env(var).filter(|v| !v.is_empty()) else {
            continue;
        };
        let candidate = PathBuf::from(base)
            .join("LibreOffice")
            .join("program")
            .join(EXE);
        let dup = out.iter().any(|p| {
            p.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&candidate.as_os_str().to_string_lossy())
        });
        if !dup {
            out.push(candidate);
        }
    }
    out
}

/// 프로필 폴더를 LibreOffice가 받는 file URL로 바꾼다.
///
/// - `C:\a b\프로필` → `file:///C:/a%20b/%ED%94%84%EB%A1%9C%ED%95%84`
/// - 역슬래시는 `/`로 바꾸고, 영숫자와 `-._~/` 그리고 드라이브 문자 뒤 `:` 말고는 UTF-8 바이트로 퍼센트 인코딩한다.
/// - `\\?\C:\…` 확장 경로 접두사는 뗀다. UNC `\\서버\공유\…`는 `file://서버/공유/…`가 된다.
pub fn profile_url(profile_dir: &Path) -> String {
    let raw = profile_dir.to_string_lossy().replace('\\', "/");
    let (prefix, rest): (&str, &str) = if let Some(r) = raw.strip_prefix("//?/UNC/") {
        ("file://", r)
    } else if let Some(r) = raw.strip_prefix("//?/") {
        ("file:///", r)
    } else if let Some(r) = raw.strip_prefix("//") {
        ("file://", r)
    } else if let Some(r) = raw.strip_prefix('/') {
        ("file:///", r)
    } else {
        ("file:///", raw.as_str())
    };
    let rest = rest.trim_end_matches('/');

    let mut url = String::with_capacity(prefix.len() + rest.len() * 3);
    url.push_str(prefix);
    let bytes = rest.as_bytes();
    let has_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    for (i, &b) in bytes.iter().enumerate() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/')
            || (b == b':' && i == 1 && has_drive);
        if keep {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

/// 입력 형식과 출력 형식에 맞는 `--convert-to` 값. 지원하지 않는 조합이면 None.
fn filter_for(input_format: &str, to: &str) -> Option<&'static str> {
    match (input_format, to) {
        ("docx" | "pptx" | "xlsx" | "odt" | "rtf", "pdf") => Some("pdf"),
        ("docx" | "odt" | "rtf", "txt") => Some("txt:Text (encoded):UTF8"),
        // 구분자 44(,) 따옴표 34(") 문자 집합 76(UTF-8). 첫 시트만 내보낸다.
        ("xlsx", "txt") => Some("csv:Text - txt - csv (StarCalc):44,34,76"),
        _ => None,
    }
}

/// soffice --headless --norestore --nolockcheck --nodefault -env:UserInstallation=<url>
///         --convert-to <filter> --outdir <out_dir> <input>
///
/// filter: pdf → "pdf", docx|odt|rtf→txt → "txt:Text (encoded):UTF8", xlsx→txt → "csv:Text - txt - csv (StarCalc):44,34,76"
/// pptx→txt처럼 필터가 없는 조합은 Err(UnsupportedFormat).
pub fn build_command(
    soffice: &Path,
    input: &Path,
    input_format: &str,
    to: &str,
    out_dir: &Path,
    profile_dir: &Path,
) -> Result<CommandSpec, CoreError> {
    let filter = filter_for(input_format, to).ok_or_else(|| CoreError::UnsupportedFormat {
        input: input_format.to_string(),
        to: Some(to.to_string()),
    })?;
    let mut env_arg = OsString::from("-env:UserInstallation=");
    env_arg.push(profile_url(profile_dir));
    let args: Vec<OsString> = vec![
        "--headless".into(),
        "--norestore".into(),
        "--nolockcheck".into(),
        "--nodefault".into(),
        env_arg,
        "--convert-to".into(),
        filter.into(),
        "--outdir".into(),
        out_dir.as_os_str().to_owned(),
        input.as_os_str().to_owned(),
    ];
    Ok(CommandSpec {
        program: soffice.to_path_buf(),
        args,
        cwd: None,
        progress: ProgressStream::None,
    })
}

/// soffice가 만들 파일 경로 (<out_dir>/<stem>.<pdf|txt|csv>)
///
/// xlsx→txt는 CSV 필터라 `.csv`로 생긴다. 최종 `.txt` 이름은 job.rs가 확정할 때 붙인다.
pub fn expected_output(input: &Path, input_format: &str, to: &str, out_dir: &Path) -> PathBuf {
    let ext = match (input_format, to) {
        ("xlsx", "txt") => "csv",
        _ => to,
    };
    let mut name = input
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_else(|| OsString::from("output"));
    name.push(".");
    name.push(ext);
    out_dir.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(spec: &CommandSpec) -> Vec<String> {
        spec.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn candidates_from_env() {
        let env = |k: &str| -> Option<OsString> {
            match k {
                "ProgramFiles" => Some(r"C:\Program Files".into()),
                "ProgramW6432" => Some(r"C:\PROGRAM FILES".into()),
                "ProgramFiles(x86)" => Some(r"C:\Program Files (x86)".into()),
                _ => None,
            }
        };
        assert_eq!(
            system_install_candidates(&env),
            vec![
                PathBuf::from(r"C:\Program Files\LibreOffice\program\soffice.exe"),
                PathBuf::from(r"C:\Program Files (x86)\LibreOffice\program\soffice.exe"),
            ]
        );
        let none = |_: &str| -> Option<OsString> { None };
        assert!(system_install_candidates(&none).is_empty());
        let empty = |_: &str| -> Option<OsString> { Some(OsString::new()) };
        assert!(system_install_candidates(&empty).is_empty());
    }

    #[test]
    fn profile_url_encoding() {
        assert_eq!(
            profile_url(Path::new(r"C:\Users\홍 길동\AppData\Local\Temp\convtool\12-3\profile")),
            "file:///C:/Users/%ED%99%8D%20%EA%B8%B8%EB%8F%99/AppData/Local/Temp/convtool/12-3/profile"
        );
        assert_eq!(profile_url(Path::new(r"C:\a b\c")), "file:///C:/a%20b/c");
        assert_eq!(
            profile_url(Path::new(r"C:\tmp\x#y%z (1)\")),
            "file:///C:/tmp/x%23y%25z%20%281%29"
        );
        assert_eq!(
            profile_url(Path::new(r"\\?\C:\긴 경로\p")),
            "file:///C:/%EA%B8%B4%20%EA%B2%BD%EB%A1%9C/p"
        );
        assert_eq!(
            profile_url(Path::new(r"\\server\share\p")),
            "file://server/share/p"
        );
        assert_eq!(profile_url(Path::new("/tmp/p q")), "file:///tmp/p%20q");
    }

    #[test]
    fn command_args_snapshot() {
        let soffice = PathBuf::from(r"C:\Program Files\LibreOffice\program\soffice.exe");
        let input = PathBuf::from(r"C:\문서 함\분기 보고서.docx");
        let out_dir = PathBuf::from(r"C:\Temp\convtool\1-2");
        let profile = PathBuf::from(r"C:\Temp\convtool\1-2\profile");
        let spec = build_command(&soffice, &input, "docx", "pdf", &out_dir, &profile).expect("pdf");
        assert_eq!(spec.program, soffice);
        assert_eq!(spec.progress, ProgressStream::None);
        assert_eq!(
            strs(&spec),
            vec![
                "--headless",
                "--norestore",
                "--nolockcheck",
                "--nodefault",
                "-env:UserInstallation=file:///C:/Temp/convtool/1-2/profile",
                "--convert-to",
                "pdf",
                "--outdir",
                r"C:\Temp\convtool\1-2",
                r"C:\문서 함\분기 보고서.docx",
            ]
        );
    }

    #[test]
    fn filter_mapping() {
        let s = Path::new("soffice.exe");
        let i = Path::new(r"C:\x\a.bin");
        let d = Path::new(r"C:\t");
        let filter = |fmt: &str, to: &str| -> Result<String, CoreError> {
            build_command(s, i, fmt, to, d, d).map(|c| strs(&c)[6].clone())
        };
        for fmt in ["docx", "pptx", "xlsx", "odt", "rtf"] {
            assert_eq!(filter(fmt, "pdf").expect("pdf"), "pdf");
        }
        for fmt in ["docx", "odt", "rtf"] {
            assert_eq!(filter(fmt, "txt").expect("txt"), "txt:Text (encoded):UTF8");
        }
        assert_eq!(
            filter("xlsx", "txt").expect("xlsx"),
            "csv:Text - txt - csv (StarCalc):44,34,76"
        );
        match filter("pptx", "txt") {
            Err(CoreError::UnsupportedFormat { input, to }) => {
                assert_eq!(input, "pptx");
                assert_eq!(to.as_deref(), Some("txt"));
            }
            other => panic!("예상하지 않은 결과: {other:?}"),
        }
        assert!(matches!(
            filter("docx", "mp4"),
            Err(CoreError::UnsupportedFormat { .. })
        ));
        assert!(matches!(
            filter("md", "pdf"),
            Err(CoreError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn expected_output_paths() {
        let d = Path::new(r"C:\Temp\convtool\1-2");
        assert_eq!(
            expected_output(Path::new(r"C:\문서 함\분기 보고서.docx"), "docx", "pdf", d),
            PathBuf::from(r"C:\Temp\convtool\1-2\분기 보고서.pdf")
        );
        assert_eq!(
            expected_output(Path::new(r"C:\x\표.v2.xlsx"), "xlsx", "txt", d),
            PathBuf::from(r"C:\Temp\convtool\1-2\표.v2.csv")
        );
        assert_eq!(
            expected_output(Path::new(r"C:\x\메모.odt"), "odt", "txt", d),
            PathBuf::from(r"C:\Temp\convtool\1-2\메모.txt")
        );
    }
}
