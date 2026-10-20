//! 실제 엔진으로 변환하는 통합 테스트. 기본으로 건너뛴다: `cargo test -- --ignored`
//!
//! 엔진은 `EngineConfig::from_env()`로 찾는다(`engines/`, `CONVTOOL_*`, PATH).
//! 엔진이 없으면 실패하지 않고 건너뛰는 이유를 출력한다.
//! 입력 파일은 ffmpeg lavfi(영상·음성·이미지)와 직접 쓴 텍스트(md·html)로 만든다.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use convtool_core::{
    EngineConfig, EngineId, EngineSet, EventSink, JobEvent, JobQueue, JobRequest, OutcomeKind,
    PresetId, QueueConfig,
};

fn engines() -> Arc<EngineSet> {
    let mut set = EngineSet::locate(&EngineConfig::from_env());
    set.probe();
    for info in set.status() {
        eprintln!(
            "{}: found={} path={:?} version={:?} notes={:?}",
            info.name, info.found, info.path, info.version, info.notes
        );
    }
    Arc::new(set)
}

fn skip(why: &str) {
    eprintln!("건너뜀: {why}");
}

/// 변환을 실행하고 (종료 코드, 결과 경로들, 이벤트)를 돌려준다.
fn convert(
    set: &Arc<EngineSet>,
    files: Vec<PathBuf>,
    to: &str,
) -> (u8, Vec<PathBuf>, Vec<JobEvent>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let e2 = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |ev: &JobEvent| {
        if let Ok(mut v) = e2.lock() {
            v.push(ev.clone());
        }
    });
    let q = JobQueue::new(Arc::clone(set), QueueConfig::default(), sink);
    let t = q
        .submit(JobRequest {
            files,
            to: to.into(),
            preset: PresetId::Web,
            out_dir: None,
            overwrite: false,
        })
        .expect("요청");
    let s = q.wait_batch(t.batch_id).expect("요약");
    for o in &s.outcomes {
        if let OutcomeKind::Failed { error } = &o.outcome {
            eprintln!("실패 {}: {}", o.input.display(), error.message);
        }
    }
    for r in &s.rejected {
        eprintln!("거절 {}: {}", r.input.display(), r.error.message);
    }
    let outputs = s
        .outcomes
        .iter()
        .filter_map(|o| match &o.outcome {
            OutcomeKind::Done { output } => Some(output.clone()),
            _ => None,
        })
        .collect();
    let ev = events.lock().map(|v| v.clone()).unwrap_or_default();
    (s.exit_code(), outputs, ev)
}

fn assert_nonempty(p: &Path) {
    let len = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    assert!(len > 0, "결과 파일이 비었거나 없음: {}", p.display());
}

fn ffmpeg_make(set: &EngineSet, args: &[&str], out: &Path) -> bool {
    let Some(ff) = set.path(EngineId::Ffmpeg) else {
        return false;
    };
    let status = Command::new(ff)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status();
    matches!(status, Ok(s) if s.success()) && out.is_file()
}

#[test]
#[ignore = "실제 FFmpeg 필요"]
fn ffmpeg_video_and_audio() {
    let set = engines();
    if !set.is_available(EngineId::Ffmpeg) {
        return skip("FFmpeg 없음");
    }
    let dir = tempfile::tempdir().expect("임시 폴더");
    let mov = dir.path().join("발표 녹화.mov");
    assert!(ffmpeg_make(
        &set,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=320x240:rate=15",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
            "-shortest",
        ],
        &mov
    ));
    for to in ["mp4", "mp3", "gif", "webm"] {
        let (code, outputs, events) = convert(&set, vec![mov.clone()], to);
        if code == 3 {
            skip(&format!("{to}: 인코더 없음"));
            continue;
        }
        assert_eq!(code, 0, "{to}");
        assert_nonempty(&outputs[0]);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Progress { .. })),
            "{to}: progress 이벤트"
        );
    }
    // 같은 변환을 다시 하면 " (1)"
    let (code, outputs, _) = convert(&set, vec![mov.clone()], "gif");
    assert_eq!(code, 0);
    assert_eq!(outputs[0], dir.path().join("발표 녹화 (1).gif"));

    let wav = dir.path().join("소리.wav");
    assert!(ffmpeg_make(
        &set,
        &["-f", "lavfi", "-i", "sine=frequency=440:duration=1"],
        &wav
    ));
    for to in ["flac", "mp3"] {
        let (code, outputs, _) = convert(&set, vec![wav.clone()], to);
        if code == 3 {
            skip(&format!("{to}: 인코더 없음"));
            continue;
        }
        assert_eq!(code, 0, "{to}");
        assert_nonempty(&outputs[0]);
    }
}

#[test]
#[ignore = "실제 libvips (+ 입력 생성용 FFmpeg) 필요"]
fn vips_images() {
    let set = engines();
    if !set.is_available(EngineId::Vips) {
        return skip("libvips 없음");
    }
    let dir = tempfile::tempdir().expect("임시 폴더");
    let png = dir.path().join("사진 원본.png");
    if !ffmpeg_make(
        &set,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=640x480",
            "-frames:v",
            "1",
        ],
        &png,
    ) {
        return skip("입력 PNG를 만들 FFmpeg 없음");
    }
    for to in ["jpg", "webp"] {
        let (code, outputs, _) = convert(&set, vec![png.clone()], to);
        assert_eq!(code, 0, "{to}");
        assert_nonempty(&outputs[0]);
    }
}

const MARKDOWN: &str = "# 제목\n\n본문 *강조* 문단입니다.\n\n- 항목 1\n- 항목 2\n";

#[test]
#[ignore = "실제 Pandoc 필요"]
fn pandoc_text() {
    let set = engines();
    if !set.is_available(EngineId::Pandoc) {
        return skip("Pandoc 없음");
    }
    let dir = tempfile::tempdir().expect("임시 폴더");
    let md = dir.path().join("메모 정리.md");
    std::fs::write(&md, MARKDOWN).expect("쓰기");
    for to in ["docx", "html"] {
        let (code, outputs, _) = convert(&set, vec![md.clone()], to);
        assert_eq!(code, 0, "{to}");
        assert_nonempty(&outputs[0]);
    }
}

#[test]
#[ignore = "실제 LibreOffice (+ Pandoc) 필요. 느림"]
fn office_documents() {
    let set = engines();
    if !set.is_available(EngineId::Office) {
        return skip("LibreOffice 없음");
    }
    let dir = tempfile::tempdir().expect("임시 폴더");
    if !set.is_available(EngineId::Pandoc) {
        return skip("입력 docx를 만들 Pandoc 없음");
    }
    let md = dir.path().join("보고서 초안.md");
    std::fs::write(&md, MARKDOWN).expect("쓰기");
    // md → pdf (pandoc → office 두 단계)
    let (code, outputs, events) = convert(&set, vec![md.clone()], "pdf");
    assert_eq!(code, 0);
    assert_nonempty(&outputs[0]);
    let started: Vec<EngineId> = events
        .iter()
        .filter_map(|e| match e {
            JobEvent::Started { engine, .. } => Some(*engine),
            _ => None,
        })
        .collect();
    assert_eq!(started, [EngineId::Pandoc, EngineId::Office]);

    // docx → pdf, txt
    let (code, outputs, _) = convert(&set, vec![md.clone()], "docx");
    assert_eq!(code, 0);
    let docx = outputs[0].clone();
    for to in ["pdf", "txt"] {
        let (code, outputs, _) = convert(&set, vec![docx.clone()], to);
        assert_eq!(code, 0, "{to}");
        assert_nonempty(&outputs[0]);
    }
}
