---
name: win32-integration
description: FileKit의 Windows 연동 구현법. 저수준 마우스 훅(WH_MOUSE_LL)으로 휠 클릭 가로채기와 다시 보내기, UI Automation과 IShellWindows·IFolderView2로 탐색기·바탕화면 파일 경로 얻기, Windows 11 탐색기 탭, 오버레이 창 설정, 트레이·토스트 알림·AUMID, 이름 있는 파이프, NSIS 설치와 HKCU 등록을 다룬다. app/src-tauri/나 installer/를 작업할 때 사용한다. 코어 변환 로직이나 React 화면에는 쓰지 않는다.
---

# Windows 연동 구현법

spec 4장(휠 클릭 트리거)과 6장(설치와 통합)을 구현할 때의 판단 기준이다. 크레이트는 `windows`(windows-rs)를 쓴다.

## 스레드 구성

| 스레드 | 하는 일 | 이유 |
| --- | --- | --- |
| 훅 스레드 | `SetWindowsHookExW(WH_MOUSE_LL)` 설치, `GetMessageW` 루프 유지 | 훅 콜백은 설치한 스레드의 메시지 루프에서 불린다. |
| 식별 스레드 (STA) | `CoInitializeEx(COINIT_APARTMENTTHREADED)`, UIA·`IShellWindows` 호출 | Shell COM은 STA가 필요하고, 훅 콜백을 막으면 안 된다. |
| Tauri 메인 | 오버레이 창 이동·표시, 이벤트 전달 | 창 조작은 창을 만든 스레드에서. |

훅 콜백 → 식별 스레드는 채널로 넘긴다. 콜백 안에서 할당·잠금·로그 쓰기를 최소화한다.

## 훅 콜백의 판정

1. `WM_MBUTTONDOWN`만 본다. `LLMHF_INJECTED`이면서 `dwExtraInfo`가 우리 표식이면 우리가 다시 보낸 입력이므로 그대로 넘긴다(무한 반복 방지).
2. `WindowFromPoint` → `GetAncestor(GA_ROOT)`로 최상위 창을 얻고 `GetClassNameW`로 분류한다.
   - 최상위가 `CabinetWClass`이고 커서 아래 창이 `DirectUIHWND` → 탐색기 후보
   - 최상위가 `Progman` 또는 `WorkerW` → 바탕화면 후보
   - 그 밖 → 즉시 `CallNextHookEx`
3. 후보이면 클릭을 삼키고(`LRESULT(1)`) 식별 스레드에 좌표와 HWND를 넘긴다. 짝이 되는 `WM_MBUTTONUP`도 삼켜야 앱에 반쪽 클릭이 가지 않는다. 버튼 상태를 기억한다.
4. 분류 함수(클래스 이름 → 후보 종류)는 Win32와 떼어 순수 함수로 만들고 테스트한다.

## 항목 식별과 원래 동작 살리기

- `IUIAutomation::ElementFromPoint`의 `ControlType`이 `ListItem`이 아니면(빈 공간, 탐색 창, 머리글) 삼켰던 클릭을 `SendInput`으로 down·up 다시 보낸다. 이때 `dwExtraInfo`에 표식을 넣는다.
- 경로: `IShellWindows`에서 HWND가 같은 창 → `IServiceProvider::QueryService(SID_STopLevelBrowser)` → `IShellBrowser::QueryActiveShellView` → `IFolderView2`로 폴더와 선택 항목을 읽는다.
- **Windows 11 탐색기 탭:** 한 `CabinetWClass` 창에 탭이 여러 개면 `IShellWindows`에 같은 HWND 항목이 여러 개 나온다. 보이는 `ShellTabWindowClass` 자식과 대응하는 활성 탭을 골라야 한다. Windows 10에는 없는 분기라 회귀 테스트를 따로 둔다.
- 바탕화면: `IShellWindows::FindWindowSW(SWC_DESKTOP)` 또는 `SHGetKnownFolderPath(FOLDERID_Desktop)`.
- 항목 이름만으로 경로를 만들지 말고 가능하면 `IShellItem`의 파싱 이름(`SIGDN_FILESYSPATH`)을 쓴다. 확장자 숨김 설정 때문에 표시 이름과 실제 이름이 다르다.
- 다중 선택(F-03): 클릭한 항목이 선택 집합에 있으면 선택 전체, 아니면 클릭한 항목만.
- 관리자 권한 창 위에서는 UIPI 때문에 동작하지 않는다. 실패를 오류로 띄우지 말고 조용히 통과시키되 로그에 남긴다.

## 오버레이 창

- Tauri 창 설정: `decorations: false`, `transparent: true`, `alwaysOnTop: true`, `skipTaskbar: true`, `visible: false`, `focus: false`. 앱 시작 시 만들어 숨겨 둔다.
- Win32 확장 스타일 `WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED`(+ 열기 전에는 `WS_EX_NOACTIVATE`)를 HWND에 직접 확인한다.
- 위치: 커서 좌표를 중심으로 놓고, 모니터 작업 영역 밖으로 나가면 안쪽으로 민다(순수 함수로 테스트). Per-Monitor V2 DPI로 물리·논리 좌표를 혼동하지 않는다.
- 메뉴가 열려 있을 때만 입력을 받는다. 키 입력(1~8, 방향키, Enter, Esc)은 오버레이 창이 포커스를 받아 처리한다. 키보드 훅으로 받지 않는다.
- 휠 클릭부터 표시까지의 시간을 `tracing`으로 남겨 100ms 목표를 측정할 수 있게 한다.

## 트레이, 알림, 파이프

- 트레이 메뉴: 열기, 원형 메뉴 일시 중지, 설정, 종료. 변환 중에는 아이콘 색을 바꾼다.
- 토스트: AUMID를 등록해야 발신자 이름이 나온다. 진행률은 같은 알림을 갱신한다. 완료 알림에 "폴더에서 보기".
- 파이프 `\\.\pipe\convtool`: `PIPE_REJECT_REMOTE_CLIENTS`, 현재 사용자만 접근. 트레이가 서버, 창과 CLI가 클라이언트. 트레이가 없으면 CLI는 직접 코어를 실행한다.
- 단일 인스턴스: 트레이 프로세스는 하나만. 두 번째 실행은 기존 인스턴스에 파이프로 요청을 넘기고 끝난다.

## 설치와 등록 (installer/)

- Tauri 번들러의 NSIS, `installMode: "currentUser"`. 모든 등록은 `HKCU`.
- 등록 항목과 경로는 spec 6장 표를 그대로 따른다. 제거 시 모두 지우고 `%APPDATA%\FileKit` 삭제 여부를 묻는다. 변환 결과 파일은 지우지 않는다.
- 레지스트리 경로·명령 문자열 생성은 순수 함수나 NSIS 매크로 한 곳에 모은다. 작성만 하고 이 환경에서 실제 레지스트리를 바꾸지 않는다.

## 수동 확인 목록 (보고에 포함)

Windows 10과 11 각각: 파일 위 휠 클릭 → 메뉴, 빈 공간 휠 클릭 → 원래 동작, 브라우저 새 탭 휠 클릭 통과, 다중 선택, 바탕화면, 탐색기 탭 여러 개(11), 관리자 권한 창 위, 화면 가장자리, 배율 150% 모니터.
