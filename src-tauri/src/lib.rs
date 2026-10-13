pub mod config;
pub mod model;
pub mod provider;
pub mod secrets;
pub mod sync;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    #[test]
    fn scaffold_ok() {
        assert_eq!(1 + 1, 2);
    }
}
