use base64::Engine;
use elsewhere_core::{network::Runtime, store::Store};
use std::sync::Arc;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_fs::FsExt;

#[tauri::command]
async fn command(
    app: tauri::AppHandle,
    runtime: State<'_, Arc<Runtime>>,
    name: String,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if name == "save_download" {
        let filename = args["filename"]
            .as_str()
            .ok_or("Missing filename")?
            .to_string();
        if filename.len() > 500 || filename.contains(['/', '\\']) {
            return Err("Invalid filename".into());
        }
        let encoded = args["data"].as_str().ok_or("Missing download")?;
        if encoded.len() > 64 * 1024 * 1024 {
            return Err("Download is too large".into());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| e.to_string())?;
        return tauri::async_runtime::spawn_blocking(move || {
            let Some(path) = app
                .dialog()
                .file()
                .set_file_name(&filename)
                .blocking_save_file()
            else {
                return Ok(serde_json::json!({"saved":false}));
            };
            let mut opts = tauri_plugin_fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            let mut file = app.fs().open(path, opts).map_err(|e| e.to_string())?;
            std::io::Write::write_all(&mut file, &bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            Ok(serde_json::json!({"saved":true}))
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    runtime.inner().command(&name, args).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let store = Store::open(&dir).map_err(std::io::Error::other)?;
            let runtime = Runtime::new(store);
            let web_runtime = runtime.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = elsewhere_core::web::start(&web_runtime, false).await {
                    eprintln!("Web backend could not start: {error}");
                }
            });
            let agent_runtime = runtime.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = elsewhere_core::agents::resume(&agent_runtime).await {
                    eprintln!("Agent connections could not resume: {error}");
                }
            });
            app.manage(runtime);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![command])
        .run(tauri::generate_context!())
        .expect("Elsewhere could not start; original data files were preserved");
}
