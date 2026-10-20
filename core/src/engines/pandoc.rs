//! Pandoc 어댑터.
//!
//! MD·HTML → DOCX·HTML만 Pandoc 단독으로 만든다. PDF는 Pandoc에 LaTeX 같은 PDF 엔진이
//! 필요하므로 `mod.rs::plan`이 pandoc(docx) → office(pdf) 두 단계로 조립한다.

use std::ffi::OsString;
use std::path::Path;

use super::{CommandSpec, ProgressStream};
use crate::error::CoreError;

pub const EXE: &str = "pandoc.exe";

/// pandoc <in> -f markdown|html -t docx|html [-s --metadata pagetitle=<stem>] --resource-path=<입력 폴더> -o <out>
///
/// to는 "docx" | "html"만. pdf는 mod.rs::plan이 pandoc(docx)→office(pdf)로 조립.
///
/// html 출력은 `-s`(완전한 문서)로 만들고 `<title>`에 파일 이름을 넣는다. `title`이 아니라
/// `pagetitle`을 쓰는 까닭: `title`은 본문 맨 위에 제목 블록을 새로 그리고 문서 YAML의
/// title을 덮어쓰지만, `pagetitle`은 `<title>` 태그만 정한다.
pub fn build_command(
    pandoc: &Path,
    input: &Path,
    input_format: &str,
    output: &Path,
    to: &str,
) -> Result<CommandSpec, CoreError> {
    let unsupported = || CoreError::UnsupportedFormat {
        input: input_format.to_string(),
        to: Some(to.to_string()),
    };
    let from = match input_format {
        "md" => "markdown",
        "html" => "html",
        _ => return Err(unsupported()),
    };
    if !matches!(to, "docx" | "html") || input_format == to {
        return Err(unsupported());
    }

    let mut args: Vec<OsString> = vec![
        input.as_os_str().to_owned(),
        "-f".into(),
        from.into(),
        "-t".into(),
        to.into(),
    ];
    if to == "html" {
        let mut meta = OsString::from("pagetitle=");
        meta.push(input.file_stem().unwrap_or_default());
        args.push("-s".into());
        args.push("--metadata".into());
        args.push(meta);
    }
    let resource_dir = match input.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.as_os_str().to_owned(),
        _ => OsString::from("."),
    };
    let mut resource = OsString::from("--resource-path=");
    resource.push(resource_dir);
    args.push(resource);
    args.push("-o".into());
    args.push(output.as_os_str().to_owned());

    Ok(CommandSpec {
        program: pandoc.to_path_buf(),
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
    fn md_to_docx() {
        let exe = PathBuf::from(r"C:\엔진\pandoc\pandoc.exe");
        let input = PathBuf::from(r"C:\메모 장\회의 기록.md");
        let out = PathBuf::from(r"C:\Temp\convtool\1-5\회의 기록.docx");
        let spec = build_command(&exe, &input, "md", &out, "docx").expect("docx");
        assert_eq!(spec.program, exe);
        assert_eq!(spec.progress, ProgressStream::None);
        assert_eq!(
            strs(&spec),
            vec![
                r"C:\메모 장\회의 기록.md",
                "-f",
                "markdown",
                "-t",
                "docx",
                r"--resource-path=C:\메모 장",
                "-o",
                r"C:\Temp\convtool\1-5\회의 기록.docx",
            ]
        );
    }

    #[test]
    fn md_to_html_is_standalone_with_title() {
        let input = PathBuf::from(r"C:\메모 장\회의 기록.md");
        let out = PathBuf::from(r"C:\메모 장\.회의 기록.2.convtool-tmp.html");
        let spec =
            build_command(Path::new("pandoc.exe"), &input, "md", &out, "html").expect("html");
        assert_eq!(
            strs(&spec),
            vec![
                r"C:\메모 장\회의 기록.md",
                "-f",
                "markdown",
                "-t",
                "html",
                "-s",
                "--metadata",
                "pagetitle=회의 기록",
                r"--resource-path=C:\메모 장",
                "-o",
                r"C:\메모 장\.회의 기록.2.convtool-tmp.html",
            ]
        );
    }

    #[test]
    fn html_to_docx_and_relative_input() {
        let spec = build_command(
            Path::new("pandoc.exe"),
            Path::new("page.html"),
            "html",
            Path::new("page.docx"),
            "docx",
        )
        .expect("docx");
        let a = strs(&spec);
        assert_eq!(a[2], "html");
        assert_eq!(a[5], "--resource-path=.");
    }

    #[test]
    fn rejects_other_targets() {
        let p = Path::new("pandoc.exe");
        let i = Path::new(r"C:\x\a.md");
        let o = Path::new(r"C:\x\a.out");
        for (fmt, to) in [
            ("md", "pdf"),
            ("md", "md"),
            ("html", "html"),
            ("docx", "html"),
        ] {
            match build_command(p, i, fmt, o, to) {
                Err(CoreError::UnsupportedFormat { input, to: t }) => {
                    assert_eq!(input, fmt);
                    assert_eq!(t.as_deref(), Some(to));
                }
                other => panic!("{fmt}→{to}: 예상하지 않은 결과 {other:?}"),
            }
        }
    }
}
