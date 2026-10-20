//! 출력 경로, " (1)" 충돌 규칙, 임시 경로, 확정(commit).
//!
//! 원본은 절대 바꾸지 않는다. 덮어쓰기는 `overwrite=true`일 때만 한다.
//! `std::fs::rename`은 Windows에서 이미 있는 대상을 덮어쓰므로, 덮어쓰기를 막으려면
//! `create_new`로 최종 이름을 먼저 차지한다.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::CoreError;
use crate::job::JobId;

/// 번호 붙이기를 포기하는 상한 (비정상 상황 방지)
const MAX_SUFFIX: u32 = 10_000;

/// <out_dir 또는 입력 폴더>/<입력 stem>.<to>
pub fn desired_output(input: &Path, out_dir: Option<&Path>, to: &str) -> PathBuf {
    let dir = out_dir
        .map(Path::to_path_buf)
        .or_else(|| input.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let stem = input.file_stem().unwrap_or_default();
    let mut name = stem.to_os_string();
    name.push(".");
    name.push(to);
    dir.join(name)
}

/// "이름.mp4" → "이름 (n).mp4". 확장자가 없으면 "이름 (n)".
fn numbered(desired: &Path, n: u32) -> PathBuf {
    let stem = desired.file_stem().unwrap_or_default();
    let mut name: OsString = stem.to_os_string();
    name.push(format!(" ({n})"));
    if let Some(ext) = desired.extension() {
        name.push(".");
        name.push(ext);
    }
    desired.with_file_name(name)
}

/// 후보 이름 목록: 원래 이름, " (1)", " (2)", …
fn candidates(desired: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    std::iter::once(desired.to_path_buf()).chain((1..=MAX_SUFFIX).map(|n| numbered(desired, n)))
}

/// "이름.mp4" → "이름 (1).mp4" → "이름 (2).mp4" … exists가 false인 첫 이름
pub fn resolve_conflict(desired: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    candidates(desired)
        .find(|p| !exists(p))
        .unwrap_or_else(|| numbered(desired, MAX_SUFFIX + 1))
}

/// 같은 폴더 임시 파일: ".<stem>.<job_id>.convtool-tmp.<ext>" (확장자 유지, job_id로 동시 작업 충돌 방지)
pub fn temp_output(final_dir: &Path, stem: &str, job_id: JobId, ext: &str) -> PathBuf {
    final_dir.join(format!(".{stem}.{job_id}.convtool-tmp.{ext}"))
}

/// 두 경로가 같은 파일을 가리키는가. 둘 다 있으면 정규화해 비교하고(대소문자·`..` 처리),
/// 아니면 글자 그대로 비교한다(Windows는 대소문자 무시).
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    if let (Ok(x), Ok(y)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        return x == y;
    }
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

fn io_err(path: &Path, source: std::io::Error) -> CoreError {
    CoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn reject_input(path: &Path) -> CoreError {
    CoreError::InvalidInput(format!(
        "출력 경로가 원본 파일과 같습니다: {}",
        path.display()
    ))
}

/// 임시 파일을 최종 이름으로 확정한다.
///
/// - overwrite=false: OpenOptions::create_new로 최종 이름을 선점(AlreadyExists면 다음 번호) → rename(tmp, 선점 파일)
/// - overwrite=true : rename(tmp, desired) (기존 파일 교체).
///
/// 어떤 경우에도 결과 경로가 입력 파일과 같으면 Err(InvalidInput).
/// 실패하면 임시 파일은 그대로 남는다(지우는 일은 호출자 몫).
pub fn commit(
    tmp: &Path,
    desired: &Path,
    input: &Path,
    overwrite: bool,
) -> Result<PathBuf, CoreError> {
    if overwrite {
        if same_file(desired, input) {
            return Err(reject_input(desired));
        }
        std::fs::rename(tmp, desired).map_err(|e| io_err(desired, e))?;
        return Ok(desired.to_path_buf());
    }

    for candidate in candidates(desired) {
        if same_file(&candidate, input) {
            // 원본은 이미 있으므로 create_new도 실패하겠지만, 명시적으로 건너뛴다.
            continue;
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(placeholder) => {
                drop(placeholder);
                if let Err(e) = std::fs::rename(tmp, &candidate) {
                    // 선점한 빈 파일을 치운다
                    let _ = std::fs::remove_file(&candidate);
                    return Err(io_err(&candidate, e));
                }
                return Ok(candidate);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            // Windows: 같은 이름의 폴더가 있거나 지우는 중인 파일이면 PermissionDenied가 난다.
            Err(e) if e.kind() == ErrorKind::PermissionDenied && candidate.exists() => continue,
            Err(e) => return Err(io_err(&candidate, e)),
        }
    }
    Err(CoreError::Internal(format!(
        "충돌 없는 출력 이름을 찾지 못했습니다: {}",
        desired.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;

    #[test]
    fn desired_output_paths() {
        assert_eq!(
            desired_output(Path::new(r"C:\영상\발표 녹화.mov"), None, "mp4"),
            PathBuf::from(r"C:\영상\발표 녹화.mp4")
        );
        assert_eq!(
            desired_output(
                Path::new(r"C:\영상\발표 녹화.mov"),
                Some(Path::new(r"D:\출력")),
                "gif"
            ),
            PathBuf::from(r"D:\출력\발표 녹화.gif")
        );
        assert_eq!(
            desired_output(Path::new("a.b.c.docx"), None, "pdf"),
            PathBuf::from("a.b.c.pdf")
        );
    }

    #[test]
    fn temp_output_name() {
        assert_eq!(
            temp_output(Path::new(r"C:\영상"), "발표 녹화", 7, "mp4"),
            PathBuf::from(r"C:\영상\.발표 녹화.7.convtool-tmp.mp4")
        );
    }

    #[test]
    fn resolve_conflict_numbers() {
        let desired = PathBuf::from(r"C:\영상\발표 녹화.mp4");
        let taken: HashSet<PathBuf> = [
            PathBuf::from(r"C:\영상\발표 녹화.mp4"),
            PathBuf::from(r"C:\영상\발표 녹화 (1).mp4"),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            resolve_conflict(&desired, &|p| taken.contains(p)),
            PathBuf::from(r"C:\영상\발표 녹화 (2).mp4")
        );
        assert_eq!(resolve_conflict(&desired, &|_| false), desired);
        // 확장자 없음
        let no_ext = PathBuf::from(r"C:\a\README");
        assert_eq!(
            resolve_conflict(&no_ext, &|p| p == no_ext.as_path()),
            PathBuf::from(r"C:\a\README (1)")
        );
    }

    fn write(p: &Path, content: &str) {
        fs::write(p, content).expect("파일 쓰기");
    }

    fn read(p: &Path) -> String {
        fs::read_to_string(p).expect("파일 읽기")
    }

    #[test]
    fn commit_without_conflict() {
        let dir = tempfile::tempdir().expect("임시 폴더");
        let input = dir.path().join("발표 녹화.mov");
        write(&input, "원본");
        let desired = desired_output(&input, None, "mp4");
        let tmp = temp_output(dir.path(), "발표 녹화", 1, "mp4");
        write(&tmp, "결과");
        let out = commit(&tmp, &desired, &input, false).expect("확정");
        assert_eq!(out, desired);
        assert_eq!(read(&out), "결과");
        assert!(!tmp.exists());
        assert_eq!(read(&input), "원본");
    }

    #[test]
    fn commit_with_existing_numbered_names() {
        let dir = tempfile::tempdir().expect("임시 폴더");
        let input = dir.path().join("발표 녹화.mov");
        write(&input, "원본");
        let desired = dir.path().join("발표 녹화.mp4");
        write(&desired, "기존");
        write(&dir.path().join("발표 녹화 (1).mp4"), "기존1");
        let tmp = temp_output(dir.path(), "발표 녹화", 2, "mp4");
        write(&tmp, "결과");
        let out = commit(&tmp, &desired, &input, false).expect("확정");
        assert_eq!(out, dir.path().join("발표 녹화 (2).mp4"));
        assert_eq!(read(&out), "결과");
        assert_eq!(read(&desired), "기존");
        assert_eq!(read(&dir.path().join("발표 녹화 (1).mp4")), "기존1");
        assert!(!tmp.exists());
    }

    #[test]
    fn commit_overwrite_replaces() {
        let dir = tempfile::tempdir().expect("임시 폴더");
        let input = dir.path().join("발표 녹화.mov");
        write(&input, "원본");
        let desired = dir.path().join("발표 녹화.mp4");
        write(&desired, "기존");
        let tmp = temp_output(dir.path(), "발표 녹화", 3, "mp4");
        write(&tmp, "결과");
        let out = commit(&tmp, &desired, &input, true).expect("덮어쓰기");
        assert_eq!(out, desired);
        assert_eq!(read(&desired), "결과");
        assert!(!dir.path().join("발표 녹화 (1).mp4").exists());
    }

    #[test]
    fn commit_rejects_input_path() {
        let dir = tempfile::tempdir().expect("임시 폴더");
        let input = dir.path().join("발표 녹화.mp4");
        write(&input, "원본");
        let tmp = temp_output(dir.path(), "발표 녹화", 4, "mp4");
        write(&tmp, "결과");

        // overwrite=true에서 입력과 같은 경로 → 거절, 원본 유지
        let err = commit(&tmp, &input, &input, true).expect_err("거절");
        assert_eq!(err.code(), crate::error::ErrorCode::InvalidInput);
        assert_eq!(read(&input), "원본");
        assert!(tmp.exists());

        // 대소문자만 다른 경로도 같은 파일
        if cfg!(windows) {
            let upper = dir.path().join("발표 녹화.MP4");
            let err = commit(&tmp, &upper, &input, true).expect_err("거절");
            assert_eq!(err.code(), crate::error::ErrorCode::InvalidInput);
            assert_eq!(read(&input), "원본");
        }

        // overwrite=false면 원본을 건너뛰고 " (1)"로
        let out = commit(&tmp, &input, &input, false).expect("확정");
        assert_eq!(out, dir.path().join("발표 녹화 (1).mp4"));
        assert_eq!(read(&input), "원본");
        assert_eq!(read(&out), "결과");
    }

    #[test]
    fn commit_missing_tmp_cleans_placeholder() {
        let dir = tempfile::tempdir().expect("임시 폴더");
        let input = dir.path().join("a.mov");
        write(&input, "원본");
        let desired = dir.path().join("a.mp4");
        let tmp = dir.path().join(".a.9.convtool-tmp.mp4");
        let err = commit(&tmp, &desired, &input, false).expect_err("임시 파일 없음");
        assert_eq!(err.code(), crate::error::ErrorCode::Io);
        assert!(!desired.exists(), "선점한 빈 파일이 남지 않아야 한다");
    }
}
