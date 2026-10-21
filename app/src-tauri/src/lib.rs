//! FileKit 트레이 앱 (`convtool-app`).
//!
//! - 항상 컴파일하는 모듈: `args`, `events`, `placement`, `session`, `busy`, `notify`(토스트 판정 + WinRT 토스트),
//!   `pipe`(디스패치, `security` 현재 사용자 전용 보안 설명자),
//!   `trigger::classify`, `theme`(순수 변환 + Windows 읽기·변경 감시). `trigger::{hook, explorer}`의 스레드 코드는
//!   `#[cfg(windows)]`이고 tauri가 필요 없다.
//! - `tauri-app` feature: `app`, `state`, `commands`, `overlay`, `tray`, `pipe::{server, client}`.
//!
//! `cargo test -p convtool-app --no-default-features`로 tauri·tokio 없이 순수 테스트를 돌린다.

#[cfg(all(feature = "tauri-app", not(windows)))]
compile_error!(
    "convtool-app의 tauri-app 기능은 Windows 전용입니다 (이름 있는 파이프, 훅, 오버레이)."
);

pub mod args;
pub mod busy;
pub mod events;
pub mod notify;
pub mod pipe;
pub mod placement;
pub mod session;
pub mod theme;
pub mod trigger;

#[cfg(feature = "tauri-app")]
pub mod app;
#[cfg(feature = "tauri-app")]
pub mod commands;
#[cfg(feature = "tauri-app")]
pub mod overlay;
#[cfg(feature = "tauri-app")]
pub mod state;
#[cfg(feature = "tauri-app")]
pub mod tray;
