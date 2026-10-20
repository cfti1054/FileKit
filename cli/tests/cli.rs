//! `convtool` 통합 테스트 (실제 엔진 없이).
//!
//! `CONVTOOL_ENGINES_ONLY=1`과 빈 `CONVTOOL_ENGINES_DIR`로 실행해 PATH·시스템 설치·저장소 engines/를
//! 찾지 않게 한다. 종료 코드 0·1·2·3과 `--json` 줄 형식을 확인한다.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const ENGINE_VARS: [&str; 4] = [
    "CONVTOOL_FFMPEG",
    "CONVTOOL_VIPS",
    "CONVTOOL_SOFFICE",
    "CONVTOOL_PANDOC",
];

fn convtool(empty_engines: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_convtool"));
    cmd.env("CONVTOOL_ENGINES_ONLY", "1")
        .env("CONVTOOL_ENGINES_DIR", empty_engines)
        .env_remove("RUST_LOG");
    for v in ENGINE_VARS {
        cmd.env_remove(v);
    }
    cmd
}

fn run(cmd: &mut Command) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = cmd.output().expect("convtool 실행");
    (
        status.code().expect("종료 코드"),
        String::from_utf8(stdout).expect("표준 출력 UTF-8"),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

fn json(s: &str) -> Value {
    serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("JSON 아님 ({e}): {s}"))
}

fn json_lines(s: &str) -> Vec<Value> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(json)
        .collect()
}

struct Env {
    engines: tempfile::TempDir,
    files: tempfile::TempDir,
}

fn env() -> Env {
    Env {
        engines: tempfile::tempdir().expect("빈 엔진 폴더"),
        files: tempfile::tempdir().expect("파일 폴더"),
    }
}

impl Env {
    fn file(&self, name: &str) -> PathBuf {
        let p = self.files.path().join(name);
        std::fs::write(&p, b"dummy").expect("파일 쓰기");
        p
    }
}

#[test]
fn formats_unsupported_is_2() {
    let e = env();
    let (code, out, _) = run(convtool(e.engines.path()).args(["formats", "a.xyz"]));
    assert_eq!(code, 2);
    let v = json(&out);
    assert_eq!(v["error"]["code"], "unsupported_format");
    assert_eq!(v["error"]["engine"], Value::Null);
}

#[test]
fn formats_mov_is_0_with_layout() {
    let e = env();
    let (code, out, err) = run(convtool(e.engines.path()).args(["formats", "발표 녹화.mov"]));
    assert_eq!(code, 0, "{err}");
    let v = json(&out);
    assert_eq!(v["files"][0]["path"], "발표 녹화.mov");
    assert_eq!(v["files"][0]["category"], "video");
    assert_eq!(v["layout"]["slot_count"], 4);
    assert_eq!(v["layout"]["slots"].as_array().map(Vec::len), Some(4));
    // mov → 형식 4개(mp4·mp3·gif·webm) > N-1 → 오른쪽 아래(2번 칸)는 더 보기
    assert_eq!(v["layout"]["slots"][2]["kind"], "more");
    assert_eq!(v["layout"]["slots"][3]["format_id"], "gif");
    assert_eq!(v["layout"]["more"], serde_json::json!(["webm"]));
    let opts = v["options"].as_array().expect("options");
    assert_eq!(opts.len(), 4);
    for o in opts {
        assert_eq!(o["available"], false);
        assert_eq!(o["unavailable"]["code"], "engine_missing");
        assert_eq!(o["unavailable"]["engine"], "ffmpeg");
    }
}

#[test]
fn formats_primary_filters_category() {
    let e = env();
    let (code, out, _) =
        run(convtool(e.engines.path()).args(["formats", "a.mov", "b.jpg", "--primary", "a.mov"]));
    assert_eq!(code, 0);
    let v = json(&out);
    assert_eq!(v["skipped"][0]["reason"], "different_category");
}

#[test]
fn convert_identity_is_2() {
    let e = env();
    let input = e.file("a.mp4");
    let (code, out, err) = run(convtool(e.engines.path())
        .args(["convert"])
        .arg(&input)
        .args(["--to", "mp4"]));
    assert_eq!(code, 2, "{err}");
    assert!(
        out.trim().is_empty(),
        "사람용 모드 표준 출력은 결과 경로만: {out}"
    );
    assert!(err.contains("지원하지 않는 형식"), "{err}");
}

#[test]
fn convert_engine_missing_json_is_3() {
    let e = env();
    let input = e.file("발표 녹화.mov");
    let (code, out, err) = run(convtool(e.engines.path())
        .args(["convert"])
        .arg(&input)
        .args(["--to", "webm", "--json"]));
    assert_eq!(code, 3, "{err}");
    let lines = json_lines(&out);
    assert_eq!(lines.len(), 2, "{out}");
    assert_eq!(lines[0]["type"], "rejected");
    assert_eq!(lines[0]["error"]["code"], "engine_missing");
    assert_eq!(lines[0]["error"]["engine"], "ffmpeg");
    let last = lines.last().expect("마지막 줄");
    assert_eq!(last["type"], "batch_done");
    assert_eq!(last["exit_code"], 3);
    assert_eq!(last["rejected"], 1);
    assert!(!out.contains('\u{feff}'), "BOM 없음");
}

/// 형식 표에 없는 대상 → 배치 전 실패. --json이면 표준 출력에 {"error": ErrorInfo} 한 줄, 종료 코드 2.
#[test]
fn convert_unknown_target_json_is_2() {
    let e = env();
    let input = e.file("a.mov");
    let (code, out, err) = run(convtool(e.engines.path())
        .args(["convert"])
        .arg(&input)
        .args(["--to", "xyz", "--json"]));
    assert_eq!(code, 2, "{err}");
    let lines = json_lines(&out);
    assert_eq!(lines.len(), 1, "{out}");
    assert_eq!(lines[0]["error"]["code"], "unsupported_format");
    assert_eq!(lines[0]["error"]["engine"], Value::Null);
    assert_eq!(
        lines[0]["error"]["message"],
        "지원하지 않는 형식입니다: mov → xyz"
    );
    assert!(!out.contains('\u{feff}'), "BOM 없음");
    assert!(err.contains("지원하지 않는 형식"), "{err}");
}

/// 사람용 모드에서는 배치 전 실패도 표준 출력이 비어 있다(오류는 stderr).
#[test]
fn convert_unknown_target_human_is_2() {
    let e = env();
    let input = e.file("a.mov");
    let (code, out, err) = run(convtool(e.engines.path())
        .args(["convert"])
        .arg(&input)
        .args(["--to", "xyz"]));
    assert_eq!(code, 2, "{err}");
    assert!(out.trim().is_empty(), "{out}");
    assert!(err.contains("지원하지 않는 형식"), "{err}");
}

/// out_dir를 만들 수 없을 때(파일이 그 자리에 있음)도 --json이면 error 한 줄, 종료 코드 1.
#[test]
fn convert_bad_out_dir_json_is_error_line() {
    let e = env();
    let input = e.file("a.mov");
    let blocker = e.file("막힘");
    let (code, out, err) = run(convtool(e.engines.path())
        .args(["convert"])
        .arg(&input)
        .args(["--to", "mp4", "--json", "--out"])
        .arg(blocker.join("하위")));
    assert_eq!(code, 1, "{err}");
    let lines = json_lines(&out);
    assert_eq!(lines.len(), 1, "{out}");
    assert_eq!(lines[0]["error"]["code"], "io");
}

#[test]
fn convert_bad_preset_is_clap_error() {
    let e = env();
    let (code, _, _) =
        run(convtool(e.engines.path())
            .args(["convert", "a.mov", "--to", "mp4", "--preset", "huge"]));
    assert_eq!(code, 2, "clap 사용 오류 기본값");
}

/// 실제로 실행된 작업이 실패하면 1. 가짜 pandoc으로 where.exe를 쓴다(인자를 이해하지 못해 실패).
#[cfg(windows)]
#[test]
fn convert_engine_failure_is_1() {
    let e = env();
    let input = e.file("메모 정리.md");
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let fake = PathBuf::from(root).join("System32").join("where.exe");
    let (code, out, err) = run(convtool(e.engines.path())
        .env("CONVTOOL_PANDOC", &fake)
        .args(["convert"])
        .arg(&input)
        .args(["--to", "docx", "--json"]));
    assert_eq!(code, 1, "{out}\n{err}");
    let lines = json_lines(&out);
    let types: Vec<&str> = lines.iter().filter_map(|l| l["type"].as_str()).collect();
    assert_eq!(types.first(), Some(&"queued"));
    assert!(types.contains(&"started"));
    assert!(types.contains(&"failed"));
    let last = lines.last().expect("마지막 줄");
    assert_eq!(last["type"], "batch_done");
    assert_eq!(last["failed"], 1);
    assert_eq!(last["exit_code"], 1);
    // 임시 파일이 남지 않는다
    let leftovers: Vec<_> = std::fs::read_dir(e.files.path())
        .expect("폴더")
        .filter_map(Result::ok)
        .map(|d| d.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "메모 정리.md")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn presets_json() {
    let e = env();
    let (code, out, _) = run(convtool(e.engines.path()).args(["presets", "--json"]));
    assert_eq!(code, 0);
    let v = json(&out);
    let arr = v.as_array().expect("배열");
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["id"], "web");
    assert!(arr[0]["label"].is_string());
}

#[test]
fn engines_json() {
    let e = env();
    let (code, out, _) = run(convtool(e.engines.path()).args(["engines", "--json"]));
    assert_eq!(code, 0);
    let v = json(&out);
    let arr = v.as_array().expect("배열");
    let ids: Vec<&str> = arr.iter().filter_map(|x| x["id"].as_str()).collect();
    assert_eq!(ids, ["ffmpeg", "vips", "office", "pandoc"]);
    for x in arr {
        assert_eq!(x["found"], false);
        assert_eq!(x["path"], Value::Null);
    }
    assert_eq!(arr[0]["required"], true);
    assert_eq!(arr[2]["required"], false);
}

#[test]
fn human_presets_and_engines() {
    let e = env();
    let (code, out, _) = run(convtool(e.engines.path()).arg("presets"));
    assert_eq!(code, 0);
    assert!(out.contains("web"));
    let (code, out, _) = run(convtool(e.engines.path()).arg("engines"));
    assert_eq!(code, 0);
    assert!(out.contains("FFmpeg"));
}
