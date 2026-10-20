//! 엔진 탐색과 probe.
//!
//! 탐색 순서: 설정 경로(overrides) → 번들 폴더 → PATH → (LibreOffice) 시스템 설치.
//! 탐색은 파일 존재만 확인하고 프로세스를 띄우지 않는다. 기능 확인은 [`EngineSet::probe`]가 한다.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::runner::capture_stdout;
use super::{ffmpeg, office, pandoc, vips, CommandSpec, EngineId, ProgressStream};

/// probe 명령 하나의 제한 시간
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineSource {
    Bundled,
    Config,
    Path,
    System,
}

#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    /// engines/ 후보 (앞이 우선)
    pub bundle_dirs: Vec<PathBuf>,
    /// 설정 경로 (지금은 환경 변수에서)
    pub overrides: BTreeMap<EngineId, PathBuf>,
    pub search_path: bool,
    pub search_system: bool,
}

/// 엔진별 설정 경로 환경 변수
fn override_var(id: EngineId) -> &'static str {
    match id {
        EngineId::Ffmpeg => "CONVTOOL_FFMPEG",
        EngineId::Vips => "CONVTOOL_VIPS",
        EngineId::Office => "CONVTOOL_SOFFICE",
        EngineId::Pandoc => "CONVTOOL_PANDOC",
    }
}

fn non_empty(v: Option<OsString>) -> Option<OsString> {
    v.filter(|s| !s.is_empty())
}

impl EngineConfig {
    /// 환경 변수: CONVTOOL_ENGINES_DIR, CONVTOOL_FFMPEG, CONVTOOL_VIPS, CONVTOOL_SOFFICE, CONVTOOL_PANDOC,
    /// CONVTOOL_ENGINES_ONLY=1 (PATH·시스템 설치 탐색 끄기, 테스트용).
    /// 기본 bundle_dirs: [ENV, <exe 폴더>/engines, <exe 폴더>/../../engines (개발: target/debug → 저장소 루트)]
    ///
    /// CONVTOOL_ENGINES_ONLY=1이고 CONVTOOL_ENGINES_DIR가 있으면 bundle_dirs는 그 폴더 하나뿐이다
    /// (테스트가 저장소의 engines/를 우연히 찾지 않게).
    pub fn from_env() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        Self::from_env_with(&|k: &str| std::env::var_os(k), exe_dir.as_deref())
    }

    /// [`from_env`](Self::from_env)의 순수 버전 (테스트용)
    pub(crate) fn from_env_with(
        env: &dyn Fn(&str) -> Option<OsString>,
        exe_dir: Option<&Path>,
    ) -> Self {
        let engines_only = non_empty(env("CONVTOOL_ENGINES_ONLY")).is_some_and(|v| v == "1");
        let env_dir = non_empty(env("CONVTOOL_ENGINES_DIR")).map(PathBuf::from);

        let mut bundle_dirs = Vec::new();
        if let Some(d) = &env_dir {
            bundle_dirs.push(d.clone());
        }
        if !(engines_only && env_dir.is_some()) {
            if let Some(exe) = exe_dir {
                bundle_dirs.push(exe.join("engines"));
                bundle_dirs.push(exe.join("..").join("..").join("engines"));
            }
        }

        let overrides = EngineId::ALL
            .iter()
            .filter_map(|&id| non_empty(env(override_var(id))).map(|p| (id, PathBuf::from(p))))
            .collect();

        EngineConfig {
            bundle_dirs,
            overrides,
            search_path: !engines_only,
            search_system: !engines_only,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub id: EngineId,
    pub name: String,
    pub found: bool,
    pub path: Option<PathBuf>,
    pub source: Option<EngineSource>,
    pub version: Option<String>,
    pub required: bool,
    /// 예: "h264_mf 사용", "HEIC 지원 없음", "ffprobe 없음: 진행률 알 수 없음"
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EngineSet {
    paths: BTreeMap<EngineId, PathBuf>,
    sources: BTreeMap<EngineId, EngineSource>,
    versions: BTreeMap<EngineId, String>,
    notes: BTreeMap<EngineId, Vec<String>>,
    ffprobe: Option<PathBuf>,
    ffmpeg_caps: Option<ffmpeg::FfmpegCaps>,
    vips_heic: Option<bool>,
}

/// 실행 파일 이름. Windows가 아니면 ".exe"를 뗀다(개발 편의).
fn exe_file(name: &str) -> &str {
    if cfg!(windows) {
        name
    } else {
        name.trim_end_matches(".exe")
    }
}

fn exe_name(id: EngineId) -> &'static str {
    exe_file(match id {
        EngineId::Ffmpeg => ffmpeg::EXE,
        EngineId::Vips => vips::EXE,
        EngineId::Office => office::EXE,
        EngineId::Pandoc => pandoc::EXE,
    })
}

/// 번들 폴더 안의 하위 폴더 이름
fn bundle_subdir(id: EngineId) -> &'static str {
    match id {
        EngineId::Ffmpeg => "ffmpeg",
        EngineId::Vips => "vips",
        EngineId::Office => "libreoffice",
        EngineId::Pandoc => "pandoc",
    }
}

/// 번들 폴더 후보: <dir>/<sub>/{exe, bin/exe, program/exe}
fn bundle_candidates(dir: &Path, id: EngineId) -> Vec<PathBuf> {
    let sub = dir.join(bundle_subdir(id));
    let exe = exe_name(id);
    vec![
        sub.join(exe),
        sub.join("bin").join(exe),
        sub.join("program").join(exe),
    ]
}

fn path_candidates(env: &dyn Fn(&str) -> Option<OsString>, exe: &str) -> Vec<PathBuf> {
    env("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join(exe)).collect())
        .unwrap_or_default()
}

/// `ffmpeg version 7.1-full_build-www.gyan.dev Copyright …` → "7.1-full_build-www.gyan.dev"
/// `vips-8.15.2-Tue Mar 12 …` → "8.15.2"
/// `pandoc 3.1.11` / `pandoc.exe 3.1.11` → "3.1.11"
pub(crate) fn parse_version(id: EngineId, stdout: &str) -> Option<String> {
    let first = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    let v = match id {
        EngineId::Ffmpeg => {
            let mut it = first.split_whitespace();
            it.find(|t| *t == "version")?;
            it.next()?.to_string()
        }
        EngineId::Vips => {
            let rest = first.strip_prefix("vips-").unwrap_or(first);
            rest.split(['-', ' ']).next()?.to_string()
        }
        EngineId::Pandoc | EngineId::Office => first.split_whitespace().nth(1)?.to_string(),
    };
    (!v.is_empty()).then_some(v)
}

fn spec(program: &Path, args: &[&str]) -> CommandSpec {
    CommandSpec {
        program: program.to_path_buf(),
        args: args.iter().map(OsString::from).collect(),
        cwd: None,
        progress: ProgressStream::None,
    }
}

impl EngineSet {
    /// 파일 존재만 확인한다. 프로세스를 띄우지 않는다.
    /// 탐색 순서: overrides → bundle_dirs/<이름>/{exe, bin/exe} → PATH → (office) 시스템 설치
    pub fn locate(cfg: &EngineConfig) -> Self {
        Self::locate_with(cfg, &|p: &Path| p.is_file(), &|k: &str| std::env::var_os(k))
    }

    /// 테스트용: 존재 판정과 환경 변수를 주입한다
    ///
    /// ffprobe는 찾은 ffmpeg와 같은 폴더에서 먼저 찾고(CONVTOOL_FFMPEG로 지정해도 같다),
    /// 없으면 search_path일 때 PATH에서 찾는다.
    pub fn locate_with(
        cfg: &EngineConfig,
        exists: &dyn Fn(&Path) -> bool,
        env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Self {
        let mut set = EngineSet::default();
        for id in EngineId::ALL {
            let mut notes = Vec::new();
            let mut found: Option<(PathBuf, EngineSource)> = None;

            if let Some(p) = cfg.overrides.get(&id) {
                if exists(p) {
                    found = Some((p.clone(), EngineSource::Config));
                } else {
                    notes.push(format!(
                        "{}에 지정한 경로에 파일이 없습니다: {}",
                        override_var(id),
                        p.display()
                    ));
                }
            }
            if found.is_none() {
                found = cfg
                    .bundle_dirs
                    .iter()
                    .flat_map(|d| bundle_candidates(d, id))
                    .find(|p| exists(p))
                    .map(|p| (p, EngineSource::Bundled));
            }
            if found.is_none() && cfg.search_path {
                found = path_candidates(env, exe_name(id))
                    .into_iter()
                    .find(|p| exists(p))
                    .map(|p| (p, EngineSource::Path));
            }
            if found.is_none() && cfg.search_system && id == EngineId::Office {
                found = office::system_install_candidates(env)
                    .into_iter()
                    .find(|p| exists(p))
                    .map(|p| (p, EngineSource::System));
            }

            if let Some((path, source)) = found {
                if id == EngineId::Ffmpeg {
                    let probe_exe = exe_file(ffmpeg::PROBE_EXE);
                    let sibling = path.parent().map(|d| d.join(probe_exe));
                    set.ffprobe = sibling.filter(|p| exists(p)).or_else(|| {
                        if cfg.search_path {
                            path_candidates(env, probe_exe)
                                .into_iter()
                                .find(|p| exists(p))
                        } else {
                            None
                        }
                    });
                    if set.ffprobe.is_none() {
                        notes.push("ffprobe 없음: 진행률 알 수 없음".into());
                    }
                }
                set.paths.insert(id, path);
                set.sources.insert(id, source);
            }
            if !notes.is_empty() {
                set.notes.insert(id, notes);
            }
        }
        set
    }

    fn note(&mut self, id: EngineId, note: impl Into<String>) {
        self.notes.entry(id).or_default().push(note.into());
    }

    /// 엔진을 실행해 버전과 기능을 확인한다: ffmpeg -version / -hide_banner -encoders, vips --version / -l,
    /// pandoc --version. office는 실행하지 않는다(느림). 실패해도 패닉하지 않고 notes에 남긴다.
    pub fn probe(&mut self) {
        if let Some(p) = self.paths.get(&EngineId::Ffmpeg).cloned() {
            self.probe_version(EngineId::Ffmpeg, &p, &["-version"]);
            match capture_stdout(&spec(&p, &["-hide_banner", "-encoders"]), PROBE_TIMEOUT) {
                Ok(out) => {
                    let caps = ffmpeg::parse_encoders(&out);
                    match caps.h264 {
                        Some(ffmpeg::H264Encoder::MediaFoundation) => {
                            self.note(EngineId::Ffmpeg, "h264_mf 사용")
                        }
                        Some(ffmpeg::H264Encoder::OpenH264) => {
                            self.note(EngineId::Ffmpeg, "libopenh264 사용")
                        }
                        None => self.note(EngineId::Ffmpeg, "H.264 인코더 없음: MP4 만들 수 없음"),
                    }
                    if !(caps.vp9 && caps.opus) {
                        self.note(EngineId::Ffmpeg, "VP9/Opus 인코더 없음: WEBM 만들 수 없음");
                    }
                    if !caps.mp3lame {
                        self.note(EngineId::Ffmpeg, "libmp3lame 없음: MP3 만들 수 없음");
                    }
                    self.ffmpeg_caps = Some(caps);
                }
                Err(e) => self.note(EngineId::Ffmpeg, format!("인코더 확인 실패: {e}")),
            }
        }
        if let Some(p) = self.paths.get(&EngineId::Vips).cloned() {
            self.probe_version(EngineId::Vips, &p, &["--version"]);
            match capture_stdout(&spec(&p, &["-l"]), PROBE_TIMEOUT) {
                Ok(out) => {
                    let heic = vips::parse_heic_support(&out);
                    if !heic {
                        self.note(EngineId::Vips, "HEIC 지원 없음");
                    }
                    self.vips_heic = Some(heic);
                }
                Err(e) => self.note(EngineId::Vips, format!("기능 확인 실패: {e}")),
            }
        }
        if let Some(p) = self.paths.get(&EngineId::Pandoc).cloned() {
            self.probe_version(EngineId::Pandoc, &p, &["--version"]);
        }
    }

    fn probe_version(&mut self, id: EngineId, program: &Path, args: &[&str]) {
        match capture_stdout(&spec(program, args), PROBE_TIMEOUT) {
            Ok(out) => match parse_version(id, &out) {
                Some(v) => {
                    self.versions.insert(id, v);
                }
                None => self.note(id, "버전을 읽지 못했습니다"),
            },
            Err(e) => self.note(id, format!("버전 확인 실패: {e}")),
        }
    }

    pub fn path(&self, id: EngineId) -> Option<&Path> {
        self.paths.get(&id).map(PathBuf::as_path)
    }

    pub fn is_available(&self, id: EngineId) -> bool {
        self.paths.contains_key(&id)
    }

    pub fn ffprobe_path(&self) -> Option<&Path> {
        self.ffprobe.as_deref()
    }

    /// None = probe 전
    pub fn ffmpeg_caps(&self) -> Option<&ffmpeg::FfmpegCaps> {
        self.ffmpeg_caps.as_ref()
    }

    /// None = probe 전
    pub fn vips_heic(&self) -> Option<bool> {
        self.vips_heic
    }

    /// EngineId::ALL 순서
    pub fn status(&self) -> Vec<EngineInfo> {
        EngineId::ALL
            .iter()
            .map(|&id| EngineInfo {
                id,
                name: id.display_name().to_string(),
                found: self.is_available(id),
                path: self.paths.get(&id).cloned(),
                source: self.sources.get(&id).copied(),
                version: self.versions.get(&id).cloned(),
                required: id.required(),
                notes: self.notes.get(&id).cloned().unwrap_or_default(),
            })
            .collect()
    }

    /// 테스트·앱용 수동 구성
    pub fn from_parts(
        paths: BTreeMap<EngineId, PathBuf>,
        ffmpeg_caps: Option<ffmpeg::FfmpegCaps>,
        vips_heic: Option<bool>,
    ) -> Self {
        EngineSet {
            paths,
            ffmpeg_caps,
            vips_heic,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn engine_info_json_shape() {
        let mut paths = BTreeMap::new();
        paths.insert(
            EngineId::Ffmpeg,
            PathBuf::from(r"C:\engines\ffmpeg\ffmpeg.exe"),
        );
        let set = EngineSet::from_parts(paths, None, Some(false));
        assert!(set.is_available(EngineId::Ffmpeg));
        assert!(!set.is_available(EngineId::Office));
        assert_eq!(set.vips_heic(), Some(false));
        let status = set.status();
        assert_eq!(status.len(), 4);
        let v = serde_json::to_value(&status[0]).expect("직렬화");
        assert_eq!(
            v,
            serde_json::json!({
                "id": "ffmpeg", "name": "FFmpeg", "found": true,
                "path": "C:\\engines\\ffmpeg\\ffmpeg.exe", "source": null, "version": null,
                "required": true, "notes": []
            })
        );
        assert_eq!(
            serde_json::to_value(EngineSource::System).expect("직렬화"),
            serde_json::json!("system")
        );
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    #[test]
    fn config_from_env_defaults() {
        let env = env_of(&[]);
        let cfg = EngineConfig::from_env_with(&env, Some(Path::new(r"C:\앱")));
        assert_eq!(
            cfg.bundle_dirs,
            vec![
                PathBuf::from(r"C:\앱").join("engines"),
                PathBuf::from(r"C:\앱")
                    .join("..")
                    .join("..")
                    .join("engines"),
            ]
        );
        assert!(cfg.overrides.is_empty());
        assert!(cfg.search_path && cfg.search_system);
    }

    #[test]
    fn config_from_env_vars() {
        let env = env_of(&[
            ("CONVTOOL_ENGINES_DIR", r"D:\엔진"),
            ("CONVTOOL_FFMPEG", r"D:\ff\ffmpeg.exe"),
            ("CONVTOOL_SOFFICE", r"E:\lo\soffice.exe"),
            ("CONVTOOL_PANDOC", ""),
        ]);
        let cfg = EngineConfig::from_env_with(&env, Some(Path::new(r"C:\앱")));
        assert_eq!(cfg.bundle_dirs[0], PathBuf::from(r"D:\엔진"));
        assert_eq!(cfg.bundle_dirs.len(), 3);
        assert_eq!(
            cfg.overrides.get(&EngineId::Ffmpeg),
            Some(&PathBuf::from(r"D:\ff\ffmpeg.exe"))
        );
        assert_eq!(
            cfg.overrides.get(&EngineId::Office),
            Some(&PathBuf::from(r"E:\lo\soffice.exe"))
        );
        assert!(!cfg.overrides.contains_key(&EngineId::Pandoc), "빈 값 무시");

        let env = env_of(&[
            ("CONVTOOL_ENGINES_DIR", r"D:\엔진"),
            ("CONVTOOL_ENGINES_ONLY", "1"),
        ]);
        let cfg = EngineConfig::from_env_with(&env, Some(Path::new(r"C:\앱")));
        assert_eq!(cfg.bundle_dirs, vec![PathBuf::from(r"D:\엔진")]);
        assert!(!cfg.search_path && !cfg.search_system);
    }

    #[test]
    fn locate_order_and_sources() {
        let bundle = PathBuf::from(r"C:\앱\engines");
        let files: HashSet<PathBuf> = [
            bundle
                .join("ffmpeg")
                .join("bin")
                .join(exe_name(EngineId::Ffmpeg)),
            bundle
                .join("ffmpeg")
                .join("bin")
                .join(exe_file(ffmpeg::PROBE_EXE)),
            bundle
                .join("vips")
                .join("bin")
                .join(exe_name(EngineId::Vips)),
            PathBuf::from(r"C:\설정\vips.exe"),
            PathBuf::from(r"C:\도구").join(exe_name(EngineId::Pandoc)),
            bundle
                .join("libreoffice")
                .join("program")
                .join(exe_name(EngineId::Office)),
        ]
        .into_iter()
        .collect();
        let mut overrides = BTreeMap::new();
        overrides.insert(EngineId::Vips, PathBuf::from(r"C:\설정\vips.exe"));
        overrides.insert(EngineId::Pandoc, PathBuf::from(r"C:\없음\pandoc.exe"));
        let cfg = EngineConfig {
            bundle_dirs: vec![PathBuf::from(r"C:\없는곳"), bundle.clone()],
            overrides,
            search_path: true,
            search_system: true,
        };
        let path_var = std::env::join_paths([PathBuf::from(r"C:\빈"), PathBuf::from(r"C:\도구")])
            .expect("PATH");
        let env = move |k: &str| (k == "PATH").then(|| path_var.clone());
        let set = EngineSet::locate_with(&cfg, &|p| files.contains(p), &env);

        let st = set.status();
        assert_eq!(st[0].source, Some(EngineSource::Bundled));
        assert_eq!(
            set.ffprobe_path(),
            Some(
                bundle
                    .join("ffmpeg")
                    .join("bin")
                    .join(exe_file(ffmpeg::PROBE_EXE))
                    .as_path()
            )
        );
        assert_eq!(st[1].source, Some(EngineSource::Config));
        assert_eq!(st[1].path, Some(PathBuf::from(r"C:\설정\vips.exe")));
        assert_eq!(st[2].source, Some(EngineSource::Bundled));
        assert_eq!(st[3].source, Some(EngineSource::Path));
        assert!(
            st[3].notes[0].contains("CONVTOOL_PANDOC"),
            "{:?}",
            st[3].notes
        );
    }

    #[test]
    fn locate_engines_only_skips_path_and_reports_missing_ffprobe() {
        let ff = PathBuf::from(r"C:\ff").join(exe_name(EngineId::Ffmpeg));
        let in_path = PathBuf::from(r"C:\도구").join(exe_name(EngineId::Vips));
        let files: HashSet<PathBuf> = [ff.clone(), in_path].into_iter().collect();
        let mut overrides = BTreeMap::new();
        overrides.insert(EngineId::Ffmpeg, ff.clone());
        let cfg = EngineConfig {
            bundle_dirs: vec![],
            overrides,
            search_path: false,
            search_system: false,
        };
        let env = |k: &str| (k == "PATH").then(|| OsString::from(r"C:\도구"));
        let set = EngineSet::locate_with(&cfg, &|p| files.contains(p), &env);
        assert_eq!(set.path(EngineId::Ffmpeg), Some(ff.as_path()));
        assert!(!set.is_available(EngineId::Vips));
        assert_eq!(set.ffprobe_path(), None);
        assert_eq!(
            set.status()[0].notes,
            vec!["ffprobe 없음: 진행률 알 수 없음".to_string()]
        );
        assert_eq!(set.status()[0].source, Some(EngineSource::Config));
    }

    #[test]
    fn version_parsing() {
        assert_eq!(
            parse_version(
                EngineId::Ffmpeg,
                "ffmpeg version n7.1-lgpl Copyright (c) 2000-2024 the FFmpeg developers\nbuilt with gcc"
            ),
            Some("n7.1-lgpl".into())
        );
        assert_eq!(
            parse_version(EngineId::Vips, "vips-8.15.2-Tue Mar 12 10:00:00 UTC 2024\n"),
            Some("8.15.2".into())
        );
        assert_eq!(
            parse_version(EngineId::Pandoc, "pandoc.exe 3.1.11\nFeatures: +server"),
            Some("3.1.11".into())
        );
        assert_eq!(parse_version(EngineId::Pandoc, ""), None);
        assert_eq!(parse_version(EngineId::Ffmpeg, "garbage"), None);
    }
}
