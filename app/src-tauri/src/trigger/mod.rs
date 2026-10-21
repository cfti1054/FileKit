//! 휠 클릭 트리거 (F-01).
//!
//! 흐름: 훅 스레드(`hook`, `WH_MOUSE_LL`) → 식별 스레드(`explorer`, STA, UIA·Shell COM) →
//! 오버레이(`overlay::open_menu`, Tauri 메인 스레드).
//!
//! - `classify`: 순수 판정 (창 클래스 분류, 우리 입력 표식, F-03 선택 집합, 버튼 상태)
//! - `hook`: 훅 스레드(`WH_MOUSE_LL` 설치·해제)와 삼킨 클릭 다시 보내기(`replay`).
//! - `explorer`: 식별 스레드(UIA·IShellWindows·IFolderView2)와 순수 판정(탭, 표시 이름 → 경로).
//!
//! F-09(단축키 트리거)는 이번 범위 밖이다. 키보드 훅(`WH_KEYBOARD_LL`)은 spec 8장에 따라 쓰지 않는다.
//! 단축키가 필요해지면 `RegisterHotKey`(전역 단축키 등록, 훅 아님)를 검토한다.

pub mod classify;
pub mod explorer;
pub mod hook;

pub use classify::{classify, is_own_injected, select_targets, HitKind, PressGate, OWN_INPUT_MARK};
