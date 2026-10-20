# installer

Tauri 번들러(NSIS)로 설치 파일을 만들 때 쓰는 훅 스크립트다. 설정은 `app/src-tauri/tauri.conf.json`의
`bundle.windows.nsis`에 있다(`installMode: "currentUser"`, `installerHooks: "../../installer/hooks.nsh"`,
`languages: ["Korean", "English"]`).

## 파일

| 파일 | 내용 |
| --- | --- |
| `hooks.nsh` | `NSIS_HOOK_POSTINSTALL`(등록)과 `NSIS_HOOK_POSTUNINSTALL`(삭제, 설정 폴더 삭제 여부 묻기). UTF-8 BOM으로 저장한다. |
| `licenses/THIRD-PARTY-NOTICES.txt` | FFmpeg·libvips·Pandoc 고지와 소스 위치. 설치 폴더 `licenses\`로 복사된다. 엔진 버전을 바꾸면 같이 고친다. |
| `../app/src-tauri/tauri.release.conf.json` | 릴리스 빌드에만 병합하는 설정. `externalBin`(convtool 사이드카)과 `resources`(`engines/`, `licenses/`). |

## 설치 폴더 배치

```
%LOCALAPPDATA%\변환툴\
├─ 변환툴.exe          트레이 앱 (cargo bin convtool-app, mainBinaryName)
├─ convtool.exe        CLI 사이드카 (binaries/convtool-<triple>.exe, 번들러가 접미사를 뗌)
├─ uninstall.exe
├─ engines\            저장소 engines/ 그대로 (VERSIONS.json, ffmpeg\bin\, vips\bin\, pandoc\). LibreOffice 없음
└─ licenses\THIRD-PARTY-NOTICES.txt
```

`convtool.exe`와 `변환툴.exe`는 `<exe 폴더>\engines`에서 엔진을 찾으므로(core `EngineConfig::from_env`) 설치본에 따로 설정이 없다.

## 등록 항목 (모두 HKCU, spec 6장)

| 기능 | 위치 | 누가 만드나 |
| --- | --- | --- |
| 시작 시 실행 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 값 `변환툴` = `"<설치 폴더>\변환툴.exe" --tray` | `hooks.nsh` |
| 우클릭 메뉴 | `HKCU\Software\Classes\*\shell\변환툴` (`MUIVerb`, `Icon`, `MultiSelectModel=Player`, `command` = `"<설치 폴더>\변환툴.exe" convert "%1" --ask`) | `hooks.nsh` |
| 알림 AUMID | `HKCU\Software\Classes\AppUserModelId\com.filekit.convtool` (`DisplayName`=`변환툴`, `IconUri`) | `hooks.nsh` |
| 제거 정보 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<identifier 또는 productName>` | Tauri NSIS 템플릿 |
| 바탕화면·시작 메뉴 바로가기 | `%USERPROFILE%\Desktop\변환툴.lnk`, 시작 메뉴 | Tauri NSIS 템플릿 |
| 휠 클릭 원형 메뉴 | 레지스트리 없음. 트레이 프로세스가 실행 중에 훅을 설치하고 종료 시 해제 | 앱 |

설치 위치 기본값은 currentUser 모드의 `%LOCALAPPDATA%\<productName>`(= `%LOCALAPPDATA%\변환툴`)이다.
spec 6장의 `%LOCALAPPDATA%\Programs\변환툴`과 다를 수 있다(확인 필요, 아래).

제거하면 위 HKCU 항목을 지우고, `%APPDATA%\변환툴`이 있으면 삭제 여부를 `MB_YESNO`로 묻는다.
무음 제거(`/S`, Tauri 업데이트 포함)는 묻지 않고 남긴다. 변환 결과 파일은 원본 폴더에 있으므로 지우지 않는다.

## 만들기

MSVC Build Tools, `x86_64-pc-windows-msvc` 툴체인, Node 22, WebView2가 있는 PC 또는 CI에서 저장소 루트로:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-installer.ps1            # 엔진 내려받기 포함
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-installer.ps1 -SkipFetch # engines/에 있는 것 그대로
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-installer.ps1 -Tag v0.1.0 -OutDir dist
```

단계(실패하면 단계 이름을 출력하고 멈춘다):

1. `scripts/check-version.ps1 [-Tag]` — `Cargo.toml`(`[workspace.package]`), `app/src-tauri/tauri.conf.json`,
   `app/ui/package.json`의 버전이 같은지, 태그가 `v<버전>`인지.
2. `scripts/fetch-engines.ps1` — `scripts/engines.manifest.json`대로 FFmpeg(LGPL)·libvips·Pandoc을 `engines/`에 받는다
   (SHA256 확인, 캐시 `.cache/engines`). 사용법은 `engines/README.md`.
   이어서 `-SkipFetch` 여부와 관계없이 두 가지를 검사한다. `engines/`에 허용 목록(ffmpeg, vips, pandoc,
   VERSIONS.json, README.md) 밖의 항목(예: `libreoffice/`, `.staging-*`)이 있으면 멈춘다. 그리고 `engines/VERSIONS.json`의
   version·sha256이 manifest와 다르면 멈춘다. 이 대조는 기록 파일만 보므로, 엔진 실행 파일을 손으로 바꾼 경우는 잡지 못한다.
3. `scripts/prepare-sidecar.ps1` — `cargo build --release --locked -p convtool-cli` 후
   `app/src-tauri/binaries/convtool-<host triple>.exe`로 복사. `$env:CARGO_TARGET_DIR`를 따른다.
4. `app/ui`에서 `npm ci` (`node_modules`가 없을 때만).
5. `app/src-tauri`에서 `../ui/node_modules/.bin/tauri build --config tauri.release.conf.json -- --locked`.
6. `<target>/release/bundle/nsis/변환툴_<ver>_x64-setup.exe`를 `dist/FileKit_<ver>_x64-setup.exe`로 복사하고
   `dist/FileKit_<ver>_x64-setup.exe.sha256`(`<hex>  <파일명>`)을 쓴다. GitHub Release가 비ASCII 자산 이름을 바꾸므로 배포 이름은 ASCII다.
7. `git diff --exit-code Cargo.lock` — 빌드가 잠금 파일을 바꾸지 않았는지.

기본 `tauri.conf.json`에는 `externalBin`·`resources`를 넣지 않는다. 넣으면 tauri-build가 파일 존재를 검사해서
엔진·사이드카가 없는 개발 환경의 `cargo build -p convtool-app`이 실패하기 때문이다.

이 저장소의 개발 PC(GNU 툴체인, `windres`·MSVC 없음)에서는 5단계에서 실패한다. 1~4단계는 여기서도 확인할 수 있다.

## 릴리스 절차

1. 세 파일의 버전을 같은 값으로 올린다: `Cargo.toml`의 `[workspace.package] version`,
   `app/src-tauri/tauri.conf.json`의 `version`, `app/ui/package.json`의 `version`
   (`app/ui/package-lock.json`의 버전도 `npm install --package-lock-only`로 맞춘다).
2. 엔진을 바꿨다면 `scripts/engines.manifest.json`과 `installer/licenses/THIRD-PARTY-NOTICES.txt`의 버전·주소를 함께 고친다.
3. `powershell -File scripts/check-version.ps1 -Tag v<버전>`이 0으로 끝나는지 확인하고 커밋한다.
4. 태그를 만들고 푸시한다: `git tag v<버전>` → `git push origin v<버전>`.
   - `.github/workflows/release.yml`이 MSVC로 빌드하고, 설치 스모크(무음 설치 → 파일·엔진·HKCU 등록 확인 → 무음 제거)를 거쳐
     GitHub Release에 `FileKit_<ver>_x64-setup.exe`와 `.sha256`을 올린다.
   - 태그에 `-`가 있으면(예: `v0.2.0-rc.1`) prerelease로 올린다. 이때 세 파일의 버전도 `0.2.0-rc.1`처럼 태그와 같은 값이어야 한다
     (`check-version.ps1 -Tag`가 검사). Tauri NSIS 번들러가 prerelease 버전(`-rc.1`)을 받아들이는지는 확인 안 됨.
5. 태그 없이 확인만 하려면 Actions에서 `release` 워크플로를 수동 실행(workflow_dispatch)한다. Release 없이 artifact만 남는다.

## 설치 후 확인 방법 (수동)

레지스트리는 **읽기만** 해서 확인한다.

```powershell
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v 변환툴
reg query "HKCU\Software\Classes\*\shell\변환툴" /s
reg query "HKCU\Software\Classes\AppUserModelId\com.filekit.convtool"
reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall" /s /f 변환툴
```

- 탐색기에서 파일 우클릭(Windows 11은 "추가 옵션 표시") → "변환툴로 변환" → 트레이가 실행 중이면 커서 위치에 원형 메뉴,
  없으면 메인 창에 그 파일이 들어간다.
- "앱 및 기능"에 변환툴이 보이고, 제거 후 위 `reg query`가 모두 "찾을 수 없음"인지.
- 제거 시 `%APPDATA%\변환툴` 삭제 질문이 나오는지, "아니요"면 폴더가 남는지.
- 관리자 권한 요청(UAC)이 한 번도 뜨지 않는지.

## 확인 필요

- `mainBinaryName`에 한글(`변환툴`)을 쓸 때 NSIS 템플릿·바로가기·Uninstall 키 이름이 정상인지(번들을 실제로 만들어 보지 못함).
- 설치 위치(`%LOCALAPPDATA%\변환툴` vs spec의 `%LOCALAPPDATA%\Programs\변환툴`).
- 구성 요소 선택 페이지(엔진·바로가기·우클릭·휠 클릭·시작 시 실행 선택)는 기본 템플릿 훅만으로는 만들 수 없다.
  커스텀 NSIS 템플릿(`bundle.windows.nsis.template`)이 필요하다. 지금은 모든 통합을 기본으로 등록한다.
