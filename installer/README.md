# installer

Tauri 번들러(NSIS)로 설치 파일을 만들 때 쓰는 훅 스크립트다. 설정은 `app/src-tauri/tauri.conf.json`의
`bundle.windows.nsis`에 있다(`installMode: "currentUser"`, `installerHooks: "../../installer/hooks.nsh"`,
`languages: ["Korean", "English"]`).

## 파일

| 파일 | 내용 |
| --- | --- |
| `hooks.nsh` | `NSIS_HOOK_POSTINSTALL`(등록)과 `NSIS_HOOK_POSTUNINSTALL`(삭제, 설정 폴더 삭제 여부 묻기). UTF-8 BOM으로 저장한다. |

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

이 저장소의 개발 환경(GNU 툴체인, `windres`·MSVC 없음)에서는 앱 빌드가 실패한다. MSVC Build Tools와
`x86_64-pc-windows-msvc` 툴체인을 갖춘 PC에서:

```powershell
cd app/ui
npm install
npx tauri build --config ../src-tauri/tauri.conf.json   # 또는 app/src-tauri에서 npx(../ui의 @tauri-apps/cli) tauri build
```

결과: `target/release/bundle/nsis/변환툴_0.1.0_x64-setup.exe`.

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
