; 변환툴 NSIS 설치 훅 (Tauri 번들러 installerHooks).
; 이 파일은 UTF-8 BOM으로 저장한다 (NSIS가 한글 문자열을 읽게).
;
; 모든 등록은 HKCU(현재 사용자)에만 한다. 관리자 권한을 요구하지 않는다.
; 등록 항목은 spec 6장 "등록 항목" 표를 따른다:
;   - 시작 시 실행: HKCU\Software\Microsoft\Windows\CurrentVersion\Run  값 "변환툴" = "<설치 폴더>\변환툴.exe" --tray
;   - 우클릭 메뉴:  HKCU\Software\Classes\*\shell\변환툴  (command = "<설치 폴더>\변환툴.exe" convert "%1" --ask)
;   - 알림 AUMID:  HKCU\Software\Classes\AppUserModelId\com.filekit.convtool  DisplayName = 변환툴
;   - 제거 정보와 바탕화면 바로가기는 Tauri NSIS 템플릿이 만든다 (installer/README.md).
;   - 휠 클릭 원형 메뉴는 레지스트리 등록이 없다 (트레이 프로세스가 실행 중에 훅을 설치하고 종료 시 해제).
;
; 레지스트리 경로와 명령 문자열은 아래 !define 한 곳에 모은다.

!define CONVTOOL_EXE "변환툴.exe"
!define CONVTOOL_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define CONVTOOL_RUN_VALUE "변환툴"
!define CONVTOOL_SHELL_KEY "Software\Classes\*\shell\변환툴"
!define CONVTOOL_SHELL_VERB "변환툴로 변환"
!define CONVTOOL_AUMID "com.filekit.convtool"
!define CONVTOOL_AUMID_KEY "Software\Classes\AppUserModelId\com.filekit.convtool"
!define CONVTOOL_DATA_DIR "$APPDATA\변환툴"

!macro NSIS_HOOK_POSTINSTALL
  ; 시작 시 트레이 실행
  WriteRegStr HKCU "${CONVTOOL_RUN_KEY}" "${CONVTOOL_RUN_VALUE}" '"$INSTDIR\${CONVTOOL_EXE}" --tray'

  ; 우클릭 메뉴 (Windows 11에서는 "추가 옵션 표시" 아래에 나온다. spec 6장)
  WriteRegStr HKCU "${CONVTOOL_SHELL_KEY}" "MUIVerb" "${CONVTOOL_SHELL_VERB}"
  WriteRegStr HKCU "${CONVTOOL_SHELL_KEY}" "Icon" '"$INSTDIR\${CONVTOOL_EXE}",0'
  ; 여러 파일을 골라도 파일마다 실행되게 한다 (기본 Document 모델은 15개를 넘으면 메뉴를 숨긴다).
  ; 트레이가 300ms 안의 요청을 한 세션으로 합친다.
  WriteRegStr HKCU "${CONVTOOL_SHELL_KEY}" "MultiSelectModel" "Player"
  WriteRegStr HKCU "${CONVTOOL_SHELL_KEY}\command" "" '"$INSTDIR\${CONVTOOL_EXE}" convert "%1" --ask'
  ; TODO: SubCommands (형식별 하위 메뉴, spec 6장 "+ SubCommands"). 지금은 원형 메뉴로 형식을 묻는다.

  ; 토스트 발신자 이름 (AUMID)
  WriteRegStr HKCU "${CONVTOOL_AUMID_KEY}" "DisplayName" "변환툴"
  WriteRegExpandStr HKCU "${CONVTOOL_AUMID_KEY}" "IconUri" "$INSTDIR\${CONVTOOL_EXE}"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "${CONVTOOL_RUN_KEY}" "${CONVTOOL_RUN_VALUE}"
  DeleteRegKey HKCU "${CONVTOOL_SHELL_KEY}"
  DeleteRegKey HKCU "${CONVTOOL_AUMID_KEY}"

  ; 사용자 설정 폴더 삭제 여부를 묻는다. 무음 제거(업데이트 포함)는 묻지 않고 남긴다.
  ; 변환 결과 파일은 원본 폴더에 있으므로 건드리지 않는다.
  IfSilent convtool_keep_data
  IfFileExists "${CONVTOOL_DATA_DIR}\*.*" 0 convtool_keep_data
  MessageBox MB_YESNO|MB_ICONQUESTION "설정과 기록 폴더(${CONVTOOL_DATA_DIR})도 삭제할까요?$\r$\n변환한 파일은 삭제되지 않습니다." /SD IDNO IDNO convtool_keep_data
  RMDir /r "${CONVTOOL_DATA_DIR}"
  convtool_keep_data:
!macroend
