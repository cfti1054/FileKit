fn main() {
    // tauri-app feature일 때만 tauri-build를 돌린다(Windows 리소스에 windres가 필요할 수 있다).
    #[cfg(feature = "tauri-app")]
    tauri_build::build();
}
