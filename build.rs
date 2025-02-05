pub fn main() {
    #[cfg(feature = "system-tray")]
    tauri_winres::WindowsResource::new()
        .set_icon("images/icon.ico")
        .compile()
        .unwrap();
}
