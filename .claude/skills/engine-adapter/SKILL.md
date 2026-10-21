---
name: engine-adapter
description: FileKit 코어의 변환 엔진 어댑터(FFmpeg, libvips, LibreOffice, Pandoc)와 형식 표(formats.rs), 프리셋(presets.rs)을 만들거나 고치는 방법. 새 입력·출력 형식 추가, 엔진 명령 인자 변경, 엔진 탐색 경로, 진행률 파싱, 엔진 실행 실패 처리를 다룰 때 사용한다. 원형 메뉴 화면이나 탐색기 연동에는 쓰지 않는다.
---

# 엔진 어댑터 작성법

엔진은 모두 자식 프로세스다. 어댑터의 일은 **(1) 엔진을 찾고, (2) 명령 인자를 만들고, (3) 진행률을 읽고, (4) 결과 파일을 안전하게 확정하는 것**이다.

## 구조

- `core/src/formats.rs`: 입력 확장자 → 분류(영상·음성·이미지·문서·텍스트) → 가능한 출력 형식과 담당 엔진. 기본 방향 배치(왼쪽 위·오른쪽 위·왼쪽 아래 = 자주 쓰는 3개, 오른쪽 아래 = 더 보기)도 여기서 정한다.
- `core/src/presets.rs`: `web`, `original`, `small` 프리셋을 엔진별 매개변수로 바꾼다.
- `core/src/engines/{ffmpeg,vips,office,pandoc}.rs`: 엔진마다 한 파일. 서로 의존하지 않으므로 파일 단위로 병렬 작업할 수 있다.
- `core/src/engines/mod.rs`: 공통 트레이트와 실행기.

어댑터마다 다음을 갖춘다. 정확한 시그니처는 설계 계획의 경계 인터페이스를 따른다.

1. **탐색(locate):** `engines/<이름>/` 번들 경로 → 설정에 저장된 경로 → `PATH` 순서로 찾는다. LibreOffice는 기존 설치본(`%ProgramFiles%\LibreOffice\program\soffice.exe` 등)도 찾는다. 못 찾으면 "엔진 없음" 오류(CLI 종료 코드 3)를 낸다.
2. **명령 생성:** 입력 경로, 출력 형식, 프리셋을 받아 실행 파일과 인자 목록을 돌려주는 **순수 함수**로 만든다. 셸 문자열을 조립하지 않는다(경로의 공백·한글·따옴표 문제). 단위 테스트의 중심이다.
3. **진행률:** 엔진 출력 한 줄을 받아 0.0~1.0 또는 "알 수 없음"을 돌려주는 순수 함수.
4. **실행:** 창을 띄우지 않는다(`CREATE_NO_WINDOW`). 작업 취소나 앱 종료 시 자식을 죽인다. 표준 오류 마지막 몇 줄을 실패 원인으로 남긴다.

## 결과 파일 확정

원본 폴더에 바로 최종 이름으로 쓰지 않는다. 엔진이 중간에 죽으면 반쯤 쓴 파일이 남기 때문이다.

1. 같은 폴더에 임시 이름(`.이름.convtool-tmp.확장자`)으로 쓴다. 엔진이 확장자로 형식을 판단하므로 확장자는 유지한다.
2. 성공하면 충돌 없는 최종 이름(` (1)` 규칙)을 고른 뒤 이름을 바꾼다. 이름 고르기와 바꾸기 사이에 다른 파일이 생기면 다음 번호로 다시 시도한다.
3. 실패하면 임시 파일을 지운다.

## 엔진별 주의점

**FFmpeg (LGPL 빌드)**
- LGPL 빌드에는 `libx264`(GPL)가 없다. MP4(H.264)는 `h264_mf`(Windows Media Foundation)를 먼저 쓰고, 없으면 `libopenh264`를 쓴다. 사용할 수 있는 인코더는 `ffmpeg -hide_banner -encoders` 결과로 판단한다.
- WEBM은 `libvpx-vp9` + `libopus`, MP3는 `libmp3lame`, AAC는 내장 `aac`.
- GIF는 `palettegen` → `paletteuse` 두 단계(또는 `split` 필터 한 번)로 색을 맞춘다.
- 진행률: `-progress pipe:1 -nostats`로 `out_time_us=`를 읽고, 길이는 `ffprobe -show_entries format=duration`으로 얻는다.
- 음성 추출(영상 → MP3): `-vn`.

**libvips (`vips.exe`)**
- 기본 변환은 `vips copy 입력 출력[Q=85]`, 크기 프리셋은 `vips thumbnail`.
- HEIC 입력은 배포 빌드에 libheif가 들어 있어야 한다. 탐색 단계에서 `vips -l`이나 시험 변환으로 확인하고, 없으면 HEIC 항목을 "사용 불가"로 표시한다.
- 진행률은 거의 즉시 끝나므로 "알 수 없음"으로 두고 완료만 알린다.

**LibreOffice (`soffice --headless`)**
- `--convert-to pdf --outdir <임시 폴더>`. 출력 이름을 지정할 수 없으므로 임시 폴더에 받아 위 확정 절차로 옮긴다.
- 사용자가 LibreOffice를 이미 열어 두었으면 headless 실행이 조용히 실패한다. 작업마다 `-env:UserInstallation=file:///<임시 프로필>`로 별도 프로필을 쓴다.
- 느리므로 동시 실행을 1개로 제한한다.

**Pandoc**
- MD·HTML → DOCX·HTML은 Pandoc 단독으로 된다.
- **→ PDF는 Pandoc만으로 안 된다.** LaTeX 같은 PDF 엔진이 필요하다. 기본 경로는 Pandoc으로 DOCX를 만든 뒤 LibreOffice로 PDF를 만드는 두 단계다(가정). 바꾸려면 설계 계획에 "확인 필요"로 올린다.

## 테스트

- 명령 생성·진행률 파싱·형식 표·이름 충돌: 일반 단위 테스트. 실제 엔진이 필요 없다.
- 실제 변환: `#[ignore]` 통합 테스트. 엔진이 없으면 건너뛰는 이유를 출력한다.
- 경로 테스트에 공백과 한글 이름(`발표 녹화.mov`)을 꼭 넣는다.
