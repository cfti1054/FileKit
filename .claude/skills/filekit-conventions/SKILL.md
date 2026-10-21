---
name: filekit-conventions
description: FileKit 저장소의 공통 규칙. spec 제약(엔진 프로세스 분리, 오프라인, 코어에 UI 없음, 훅 콜백 경량, 키보드 훅 금지, HKCU 전용, 원본 덮어쓰기 금지), 크레이트·폴더 소유 영역, CLI 종료 코드와 JSON 이벤트, 오류 처리, 테스트·검사 명령, 커밋 형식을 정한다. 이 저장소에서 코드를 쓰거나 검토하는 모든 에이전트가 작업 전에 읽는다.
---

# FileKit 공통 규칙

설계 기준은 저장소 루트의 `convert-tool-dev-spec.md`다. 규칙과 spec이 어긋나면 spec을 따르고, 어긋남을 보고한다.

## 제약 (spec 근거)

이유를 함께 적었다. 예외가 필요해 보이면 이유를 기준으로 판단하고, 임의로 넘지 말고 보고한다.

| 제약 | 이유 | spec |
| --- | --- | --- |
| 엔진(FFmpeg, vips, soffice, pandoc)은 항상 자식 프로세스로 실행한다. 라이브러리로 링크하지 않는다. | 엔진이 죽어도 트레이·탐색기가 살아야 하고, FFmpeg LGPL 조건을 지키기 위해서다. | 2, 3, 8장 |
| 변환 중 네트워크를 쓰지 않는다. 네트워크는 업데이트 확인과 LibreOffice 첫 내려받기에만 쓴다. | 오프라인·개인정보 목표. | 2장 |
| `core/`에 UI, Tauri, Win32 UI 코드를 넣지 않는다. | 같은 코어를 CLI와 웹 서버가 다시 쓴다. | 3장 |
| 저수준 마우스 훅 콜백 안에서는 창 클래스 확인만 한다. UIA·COM은 다른 스레드에서 한다. | 콜백이 늦으면 Windows가 훅을 끊는다. | 4장 |
| 키보드 훅을 쓰지 않는다. | 백신 오탐 대응. 메뉴 키 입력은 오버레이 창이 받는다. | 8장 |
| 레지스트리와 설치는 `HKCU`와 사용자 폴더에만 한다. | 관리자 권한 없이 설치·실행. | 2, 6장 |
| 파일이 아닌 곳의 휠 클릭은 그대로 통과시킨다. | 간섭 최소화. | 2, 4장 |
| 출력은 원본 폴더에 쓰고, 이름이 겹치면 ` (1)`, ` (2)` …를 붙인다. `--overwrite`가 있을 때만 덮어쓴다. 원본은 절대 바꾸지 않는다. | F-04. | 2, 7장 |
| 휠 클릭부터 메뉴 표시까지 100ms 이내. 오버레이 창은 미리 만들어 숨겨 둔다. | 반응성 목표. 측정 없이 "충분히 빠르다"고 판단하지 않는다. | 2, 4장 |

## 저장소 구조와 소유 영역

병렬 작업이 서로 덮어쓰지 않도록 에이전트마다 고칠 수 있는 경로를 정했다. 자기 영역 밖을 고쳐야 하면 고치지 말고 보고한다.

| 경로 | 내용 | 소유 |
| --- | --- | --- |
| `Cargo.toml` (워크스페이스) | 멤버, 공통 의존성 버전 | core-dev |
| `core/` | 크레이트 `convtool-core` (lib `convtool_core`) | core-dev |
| `cli/` | 크레이트 `convtool-cli`, 바이너리 `convtool` | core-dev |
| `app/src-tauri/` | 크레이트 `convtool-app`, 트레이·훅·오버레이·파이프 서버 | win-dev |
| `installer/` | NSIS 훅 스크립트, 레지스트리 등록 | win-dev |
| `app/ui/` | React + TS + Vite. `src/main/`, `src/radial/`, `src/shared/` | ui-dev |
| `engines/` | 번들할 엔진 실행 파일. 커밋하지 않음 (`.gitignore`) | 없음 |
| `CLAUDE.md`, `.claude/`, `.gitignore`, `_workspace/` | 하네스와 중간 산출물 | 오케스트레이터(메인) |

워크스페이스의 `default-members`는 `core`, `cli`다. `app/src-tauri`는 멤버지만 MSVC·WebView2가 없는 환경에서도 `cargo test`가 돌도록 기본 빌드에서 뺀다.

## 공통 인터페이스 규칙

- 이름 있는 파이프: `\\.\pipe\convtool`. 메시지는 줄 단위 JSON(`\n` 구분), 필드 이름은 `snake_case`, 모든 메시지에 `"type"` 태그.
- CLI 종료 코드: `0` 성공, `1` 일부 실패, `2` 지원하지 않는 형식, `3` 엔진 없음. 그 밖의 사용 오류(인자 잘못)는 clap 기본값을 따른다.
- `--json`: 표준 출력에 한 줄에 이벤트 하나. 사람이 읽을 메시지는 표준 오류로 보낸다. 배치가 만들어졌으면 마지막 줄은 `batch_done`이고, 배치를 만들기 전에 실패하면 `{"error": ErrorInfo}` 한 줄을 쓴다. 형식 표에 없는 `--to`는 종료 코드 2다.
- 형식 ID는 소문자 확장자 문자열(`"mp4"`, `"pdf"`)로 통일한다.
- 원형 메뉴 기본 배치는 spec 5장을 따른다: 오른쪽 아래(N=4 → 2번 칸)는 항목이 N-1개보다 많으면 항상 "더 보기"이고, 그 밖의 칸은 자주 쓰는 형식 순서로 채운다. 배치 계산은 Rust(`formats::default_layout`)만 하고 화면은 받은 대로 그린다.

## Rust 코드 규칙

- 에디션 2021, 워크스페이스 공통 의존성은 `[workspace.dependencies]`에 버전을 둔다.
- 오류: 라이브러리는 `thiserror`, 바이너리는 `anyhow`. 비테스트 코드에서 `unwrap()`·`expect()`를 쓰지 않는다. 불변식이면 `expect("이유")`만 허용한다.
- 로그: `tracing`. `println!`은 CLI 출력에만 쓴다.
- Windows 전용 코드는 `#[cfg(windows)]`로 감싸고, 판정 로직은 플랫폼과 무관한 순수 함수로 떼어 테스트한다.

## TypeScript 코드 규칙

- `strict: true`. `any`를 쓰지 않는다.
- Tauri IPC 타입과 호출은 `app/ui/src/shared/ipc.ts` 한 곳에 둔다.

## 검사 명령

| 대상 | 명령 |
| --- | --- |
| Rust 포맷 | `cargo fmt --all --check` |
| Rust 린트 | `cargo clippy --workspace --all-targets -- -D warnings` (앱을 빌드할 수 없는 환경이면 `-p convtool-core -p convtool-cli`) |
| Rust 테스트 | `cargo test` (기본 멤버), 실제 엔진 테스트는 `cargo test -- --ignored` |
| UI | `app/ui/`에서 `npm run typecheck`, `npm run lint`, `npm test`, `npm run build` |

`cargo`를 찾지 못하면 Bash 명령 앞에 `export PATH="$HOME/.cargo/bin:$PATH";`를 붙인다(rustup 설치 직후 세션은 PATH가 갱신되지 않는다). 이 환경의 Rust 기본 툴체인은 `x86_64-pc-windows-gnu`다. Tauri 앱 빌드에는 MSVC Build Tools와 WebView2가 필요할 수 있다. 실패하면 오류 원문을 보고한다.

## 커밋

- 커밋과 푸시는 사용자가 요청할 때만 한다.
- 형식: `<type>(<영역>): <한국어 설명> (F-xx)` — type은 `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `build`. 영역은 `core`, `cli`, `app`, `ui`, `installer`, `harness`.
