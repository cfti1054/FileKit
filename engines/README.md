# engines/ — 변환 엔진 두는 곳

FileKit(convtool)은 변환 엔진을 라이브러리로 링크하지 않고 **자식 프로세스로 실행**한다.
엔진 실행 파일은 저장소에 커밋하지 않는다(`.gitignore`가 이 README만 남기고 `engines/` 안을 모두 뺀다).
개발할 때는 아래대로 내려받아 이 폴더에 풀고, 설치본에서는 설치 폴더의 `engines/`에 같은 배치로 들어간다.

## 폴더 배치

```
engines/
├─ ffmpeg/
│  ├─ ffmpeg.exe          (또는 bin/ffmpeg.exe)
│  └─ ffprobe.exe         (또는 bin/ffprobe.exe)  ← 없으면 진행률을 "알 수 없음"으로 표시
├─ vips/
│  └─ bin/vips.exe        (같은 bin/ 폴더의 DLL도 함께 둔다)
├─ pandoc/
│  └─ pandoc.exe
└─ libreoffice/           (선택. 없으면 시스템에 설치된 LibreOffice를 쓴다)
   └─ program/soffice.exe
```

각 엔진은 `engines/<이름>/<exe>`와 `engines/<이름>/bin/<exe>` 두 곳을 본다.
그래서 내려받은 zip의 맨 위 폴더 이름만 `ffmpeg`, `vips`, `pandoc`으로 바꿔 넣으면 된다.

## 스크립트로 받기 (권장)

`scripts/fetch-engines.ps1`이 `scripts/engines.manifest.json`에 적힌 고정 버전 zip을 받아
SHA256을 확인하고 위 배치로 풀어 둔다. 릴리스 빌드(`scripts/build-installer.ps1`)도 이 스크립트를 쓴다.

```powershell
# 저장소 루트에서. PowerShell 5.1(powershell.exe)과 7(pwsh) 둘 다 된다.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fetch-engines.ps1
pwsh -File scripts/fetch-engines.ps1 -Only ffmpeg,vips     # 일부만
pwsh -File scripts/fetch-engines.ps1 -Force                # 캐시를 무시하고 다시 받기
```

| 인자 | 뜻 |
| --- | --- |
| `-Manifest <path>` | manifest 경로. 기본 `scripts/engines.manifest.json` |
| `-Dest <path>` | 배치할 폴더. 기본 `<저장소>/engines` |
| `-Only a,b` | 이 엔진만 처리 (`ffmpeg`, `vips`, `pandoc`) |
| `-Cache <path>` | zip 캐시. 기본 `$env:CONVTOOL_ENGINE_CACHE`, 없으면 `<저장소>/.cache/engines` |
| `-Force` | 캐시를 무시하고 다시 받기 |
| `-PrintHashes` | 검증·배치 없이 zip의 SHA256만 출력 (manifest 갱신용) |

종료 코드: `0` 성공, `1` 내려받기·압축 풀기·manifest 오류, `2` SHA256 불일치, `3` 배치 검사 실패(필요한 파일 없음).

하는 일:

- 캐시에 같은 zip이 있고 SHA256이 맞으면 다시 받지 않는다. 다르면 다시 받고, 그래도 다르면 종료 코드 2로 멈춘다
  (받은 파일은 캐시에 `.mismatch`를 붙여 남긴다).
- zip은 시스템 임시 폴더에 푼다. 임시 폴더 밖을 가리키는 항목 이름(`..`, 절대 경로, 드라이브 문자, `:`)이 있으면 거부한다.
  맨 위 폴더를 떼고 manifest의 `keep`에 맞는 파일만 남긴 뒤, `required` 파일이 있는지 확인하고서야
  `engines/<이름>/`을 지우고 새로 만든다. 확인에 실패하면 기존 폴더는 그대로 둔다.
- 스크립트는 받은 실행 파일을 실행하지 않는다. 동작 확인은 아래 "확인 명령"으로 한다.
- `engines/README.md`와 `engines/libreoffice/`는 건드리지 않는다.
- 끝나면 `engines/VERSIONS.json`에 엔진별 `version`, `url`, `sha256`, `license`, `source_url`을 쓴다. 설치본에도 같이 들어간다.

manifest 규칙:

- URL은 고정 버전만 쓴다. 주소에 `latest`가 들어가면 거부한다. FFmpeg는 파일 이름에 `win64-lgpl`이 있어야 하고
  `-gpl`이나 `shared`가 있으면 거부한다.
- `keep`은 맨 위 폴더를 뗀 뒤의 상대 경로 glob이다(`*`는 한 단계, `**`는 여러 단계). 비우면 전부 남긴다.
- 버전을 올릴 때: `url`·`version`을 바꾸고 `sha256`을 비운 채
  `scripts/fetch-engines.ps1 -PrintHashes -Only <이름>`으로 값을 구해 적는다.
  GitHub 릴리스 자산은 API(`https://api.github.com/repos/<소유자>/<저장소>/releases`)의 `digest` 값과도 비교한다.
- BtbN FFmpeg-Builds는 오래된 autobuild 릴리스를 지운다. 고정한 URL이 404가 되면 새 autobuild로 manifest를 갱신해야 한다.

스크립트 파일은 UTF-8 BOM으로 저장되어 있다. BOM이 없으면 PowerShell 5.1이 한글을 잘못 읽으므로 편집기 설정을 유지한다.

## 직접 받기 (스크립트를 못 쓸 때)

아래 "엔진별로 구하는 곳"에서 zip을 받아 위 배치대로 푼다. 버전은 `scripts/engines.manifest.json`과 맞추는 것이 좋다.

## 엔진을 찾는 순서

1. 환경 변수로 지정한 실행 파일 경로 (아래 표)
2. 번들 폴더: `CONVTOOL_ENGINES_DIR` → `<exe 폴더>/engines` → `<exe 폴더>/../../engines`
   (개발 중 `target/debug/convtool.exe`에서 저장소 루트의 이 폴더를 가리킨다)
3. `PATH`
4. LibreOffice만: 시스템 설치본 `%ProgramFiles%\LibreOffice\program\soffice.exe`,
   `%ProgramW6432%\…`, `%ProgramFiles(x86)%\LibreOffice\program\soffice.exe`

| 환경 변수 | 뜻 |
| --- | --- |
| `CONVTOOL_ENGINES_DIR` | 번들 폴더(이 `engines/`와 같은 배치)의 위치 |
| `CONVTOOL_FFMPEG` | `ffmpeg.exe` 전체 경로 (`ffprobe.exe`는 같은 폴더에 두는 것을 전제로 한다) |
| `CONVTOOL_VIPS` | `vips.exe` 전체 경로 |
| `CONVTOOL_SOFFICE` | `soffice.exe` 전체 경로 |
| `CONVTOOL_PANDOC` | `pandoc.exe` 전체 경로 |
| `CONVTOOL_ENGINES_ONLY=1` | `PATH`와 시스템 설치본을 찾지 않는다 (테스트용) |

## 확인 명령

```
convtool engines            # 사람이 읽는 표
convtool engines --json     # 엔진마다 found, path, source, version, notes
```

개발 중에는 `cargo run -p convtool-cli -- engines`. notes에 "h264_mf 사용", "HEIC 지원 없음",
"ffprobe 없음: 진행률 알 수 없음" 같은 기능 확인 결과가 나온다.

## 엔진별로 구하는 곳

URL은 2026-10-20에 GitHub API와 웹 요청으로 열리는지 확인했다. 표시한 버전은 그날의 최신 릴리스다.

### FFmpeg — 필수, **LGPL 빌드만**

- 받는 곳: BtbN FFmpeg-Builds 릴리스 <https://github.com/BtbN/FFmpeg-Builds/releases> (확인함)
- 받을 파일: 이름에 **`win64-lgpl`**이 들어간 zip.
  예: `ffmpeg-master-latest-win64-lgpl.zip`, 또는 고정 버전 `ffmpeg-n8.1-latest-win64-lgpl-8.1.zip` (확인함)
  - `latest` 태그의 파일은 매일 바뀐다. 스크립트는 `autobuild-<날짜>` 태그의 파일을 고정해 쓴다
    (예: `autobuild-2026-10-19-13-06/ffmpeg-n8.1.3-18-g46d899d7a0-win64-lgpl-8.1.zip`).
  - `-shared`가 붙은 zip은 DLL을 따로 둔다. 둘 다 동작하지만 정적 빌드(이름에 `-shared` 없음)가 다루기 쉽다.
- zip 안의 `bin/ffmpeg.exe`, `bin/ffprobe.exe`를 `engines/ffmpeg/bin/`에 둔다.
- **gyan.dev 빌드와 BtbN의 `gpl` 빌드는 쓰지 않는다.** GPL 구성(libx264 등)이라 배포 조건이 바뀐다.
  LGPL 빌드에는 libx264가 없으므로 MP4(H.264)는 `h264_mf`(Windows Media Foundation)를 먼저 쓰고,
  없으면 `libopenh264`를 쓴다. 둘 다 없으면 MP4 항목이 "사용 불가"로 표시된다.
- 필요한 인코더: `h264_mf` 또는 `libopenh264`, `aac`(내장), `libvpx-vp9`, `libopus`, `libmp3lame`.
  manifest의 `n8.1.3-18-g46d899d7a0` LGPL 빌드는 `convtool engines`에서 "h264_mf 사용"만 나오고
  VP9/Opus·MP3 인코더 없음 경고가 없었다(2026-10-20 확인). 버전을 바꾸면 `convtool engines`로 다시 확인한다.
- 배포할 때는 LGPL 고지와 FFmpeg 소스 위치 안내를 함께 넣어야 한다(넣을 위치는 번들 단계에서 정한다).

### libvips — 필수

- 받는 곳: <https://github.com/libvips/build-win64-mxe/releases> (확인함, 최신 v8.18.7)
- 받을 파일: **`vips-dev-x64-all-<버전>.zip`** (예: `vips-dev-x64-all-8.18.7.zip`, 확인함).
  설계 계획에 적힌 `vips-dev-w64-all`은 옛 이름이고, 지금 릴리스의 64비트 자산 이름은 `x64`다.
  `web` 변형은 형식이 줄어든 빌드라 HEIC 등이 빠질 수 있으므로 `all`을 쓴다.
- zip의 맨 위 폴더(8.18.7은 `vips-dev-8.18`, 2026-10-20 확인)를 `engines/vips`로 바꿔 두어 `engines/vips/bin/vips.exe`가 되게 한다.
  `bin/`의 DLL과 `bin/vips-modules-<버전>/`(jxl, magick, openslide, poppler 모듈)을 옮기거나 지우지 않는다.
  스크립트는 `bin/`, `LICENSE`, `README.md`, `versions.json`만 남긴다(`include/`, `lib/`, `share/`, `etc/`는 개발용·글꼴 설정이라 뺀다).
  `versions.json`에 함께 든 라이브러리 버전이 적혀 있다.
- HEIC/HEIF 입력은 libheif가 들어 있어야 읽는다. `vips -l`에 `heifload`가 있는지로 판단하고,
  없으면 HEIC 항목을 "사용 불가"로 표시한다.
- **확인 필요:** 8.18.7 `all` zip에는 `libheif.dll`(1.23.5)과 AV1용 `libaom.dll`은 있지만 `versions.json`에
  libde265(HEVC 디코더)가 없고 DLL도 없다. 그래서 `heifload`가 있어 `convtool engines`가 HEIC를 지원한다고 보더라도
  실제 HEIC(HEVC) 파일은 못 읽고 AVIF만 읽을 수 있다. HEIC 표본 파일로 변환해 보고 판단해야 한다.
- **확인 필요(특허):** HEIC는 HEVC 코덱을 쓴다. HEVC 디코더(libde265 등)를 함께 배포할 때의
  특허·라이선스 문제를 배포 전에 확인해야 한다. libvips 자체는 LGPL-2.1이다.

### Pandoc — 선택 (MD·HTML 변환)

- 받는 곳: 공식 릴리스 <https://github.com/jgm/pandoc/releases> (확인함, 최신 3.12.1),
  설치 안내 <https://pandoc.org/installing.html> (확인함)
- 받을 파일: `pandoc-<버전>-windows-x86_64.zip` (예: `pandoc-3.12.1-windows-x86_64.zip`, 확인함). msi가 아니라 zip.
- zip 안의 `pandoc.exe`를 `engines/pandoc/pandoc.exe`에 둔다.
- Pandoc만으로는 PDF를 만들지 않는다(LaTeX가 필요하다). MD·HTML → PDF는 Pandoc으로 DOCX를 만든 뒤
  LibreOffice로 PDF를 만든다. 그래서 PDF 출력에는 LibreOffice도 있어야 한다.
- 라이선스: GPL-2.0 이상. 별도 프로세스로 실행하고 실행 파일을 그대로 배포한다.

### LibreOffice — 선택 (문서 변환, MD·HTML → PDF)

- 기본은 **이미 설치된 LibreOffice**를 쓴다(위 "찾는 순서" 4번).
- 설치본이 없으면 공식 설치 파일로 설치한다: <https://www.libreoffice.org/download/download-libreoffice/> (확인함)
- 휴대용으로 쓰려면 설치된 LibreOffice 폴더 구조 그대로 `engines/libreoffice/`에 두어
  `engines/libreoffice/program/soffice.exe`가 되게 한다. 이 배치로 실제 변환이 되는지는 **확인 안 됨**.
- 사용자가 LibreOffice를 열어 두어도 변환이 되도록 작업마다 임시 프로필
  (`-env:UserInstallation=file:///…`)을 쓴다. 느리므로 한 번에 하나만 실행한다.
- 라이선스: MPL-2.0.

## 주의

- 이 폴더의 실행 파일은 커밋하지 않는다. `git status`에 `engines/` 안의 파일이 보이면 `.gitignore`를 확인한다.
- 변환 중에는 네트워크를 쓰지 않는다. 엔진을 내려받는 일은 개발자나 설치 과정이 미리 한다.
