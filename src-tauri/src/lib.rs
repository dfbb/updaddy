pub mod adapters;
pub mod core;
pub mod disk_usage;
pub mod executor;
pub mod persistence;
pub mod proxy;
pub mod workers;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default().run(tauri::generate_context!())
}
