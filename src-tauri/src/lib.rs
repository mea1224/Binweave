mod commands;
pub mod handoff;
pub mod mcp_probe;
pub mod model;
pub mod orchestrator;
pub mod process;
pub mod role_skills;
pub mod storage;
mod worker;

use std::{path::PathBuf, sync::Arc};
use storage::Store;
use tauri::Manager;

pub(crate) struct AppState {
    store: Arc<Store>,
    workspace: PathBuf,
    diagnostics: tokio::sync::Mutex<()>,
    workers: Arc<worker::Workers>,
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let directory = app.path().app_data_dir()?;
            let workspace = std::env::current_dir()?;
            let workspace = if workspace.file_name().and_then(|s| s.to_str()) == Some("src-tauri") {
                workspace.parent().unwrap_or(&workspace).to_path_buf()
            } else {
                workspace
            };
            let store = Store::open(&directory).map_err(std::io::Error::other)?;
            // Recovery occurs once per process, never on frontend refresh.
            store.recover_interrupted().map_err(std::io::Error::other)?;
            app.manage(AppState {
                store: Arc::new(store),
                workspace,
                diagnostics: tokio::sync::Mutex::new(()),
                workers: Arc::new(worker::Workers::default()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::manage_workspace,
            commands::inspect_environment,
            commands::refresh_snapshot,
            commands::delete_history,
            commands::submit_prompt,
            commands::cancel_prompt,
            commands::respond_interaction,
            commands::set_elicitation_policy,
            commands::reset_agent_session,
            commands::swap_roles,
            commands::set_workspace,
            commands::pause_run,
            commands::acknowledge_recovery
        ])
        .run(tauri::generate_context!())
        .expect("Unable to run Binweave");
}
