# FileKit

Windows 탐색기에서 파일을 **마우스 휠로 클릭**하면 커서 위치에 원형 메뉴가 뜨고, 고른 형식으로 바로 변환하는 프로그램이다.
동영상·음성(FFmpeg), 이미지(libvips), 문서(LibreOffice, Pandoc)를 다룬다. 변환은 모두 내 PC에서 하고, 인터넷으로 파일을 보내지 않는다.
결과 파일은 원본과 같은 폴더에 만들고, 원본은 바꾸지 않는다.

## 받기와 설치

1. [Releases](https://github.com/cfti1054/FileKit/releases)에서 최신 `FileKit_<버전>_x64-setup.exe`를 받는다.
2. (권장) 받은 파일이 올린 파일과 같은지 확인한다. PowerShell에서:

   ```powershell
   (Get-FileHash .\FileKit_0.1.0_x64-setup.exe -Algorithm SHA256).Hash.ToLower()
   ```

   결과가 같은 릴리스의 `FileKit_<버전>_x64-setup.exe.sha256` 파일 앞부분(64자리)과 같아야 한다.
3. 받은 파일을 실행한다. **관리자 권한이 필요 없다.** 현재 사용자 폴더에 설치되고(Tauri currentUser 기본값 `%LOCALAPPDATA%\FileKit`, 첫 CI 설치에서 확인 예정),
   레지스트리도 현재 사용자(`HKCU`)에만 등록한다.

### "Windows의 PC 보호" (SmartScreen) 경고가 뜨면

설치 파일에 코드 서명이 없어서 처음 받은 사람에게는 경고가 뜬다. 위 SHA256을 확인한 뒤
**추가 정보 → 실행**을 누른다.

## 쓰기

- 탐색기나 바탕화면에서 파일 위에 휠 클릭 → 원형 메뉴에서 형식 선택 (숫자 키 1~8, 방향키, Enter도 된다).
- 우클릭 → (Windows 11은 "추가 옵션 표시") → "FileKit으로 변환".
- 트레이 아이콘에서 열기, 원형 메뉴 일시 중지, 설정, 종료.
- 명령줄: 설치 폴더의 `convtool.exe convert <파일> --to <형식>`.

### LibreOffice는 따로 설치

문서 변환(DOCX·PPTX·XLSX → PDF 등)과 MD·HTML → PDF에는 LibreOffice가 필요하다. 용량이 커서 설치 파일에 넣지 않았다.
[LibreOffice 공식 사이트](https://www.libreoffice.org/download/download-libreoffice/)에서 설치하면 자동으로 찾는다.
FFmpeg(LGPL 빌드), libvips, Pandoc은 설치 파일에 들어 있다.

## 제거

"설정 → 앱 → 설치된 앱"(Windows 10은 "앱 및 기능")에서 FileKit을 제거한다.
시작 시 실행, 우클릭 메뉴 등록이 함께 지워지고, 설정 폴더(`%APPDATA%\FileKit`)를 지울지 묻는다. 변환한 파일은 지우지 않는다.

## 라이선스 고지

함께 배포하는 FFmpeg·libvips·Pandoc의 라이선스와 소스 위치는
[`installer/licenses/THIRD-PARTY-NOTICES.txt`](installer/licenses/THIRD-PARTY-NOTICES.txt)
(설치 폴더의 `licenses\THIRD-PARTY-NOTICES.txt`)에 있다.

## 개발자

- 설계: [`convert-tool-dev-spec.md`](convert-tool-dev-spec.md)
- 엔진 준비: [`engines/README.md`](engines/README.md)
- 설치 파일 만들기와 릴리스 절차: [`installer/README.md`](installer/README.md) (`scripts/build-installer.ps1`)
- 구성: `core/`(변환 코어), `cli/`(`convtool.exe`), `app/src-tauri/`(트레이·휠 클릭·오버레이), `app/ui/`(원형 메뉴·메인 창)
