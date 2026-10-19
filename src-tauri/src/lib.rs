use tauri::{Url, WebviewUrl, WebviewWindowBuilder};

/// The live mapper. Phase 0 step 3 turns this into a setting (live / test).
pub const DEFAULT_SERVER: &str = "https://devilsrage.duckdns.org";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let server: Url = DEFAULT_SERVER.parse()?;

            // The main window is created in code (not in tauri.conf.json) so the
            // server address can come from settings later without a rebuild.
            // The mapper page is remote content: it gets NO access to the app's
            // native commands. Native features will be exposed deliberately, per
            // command, when Phase 1 needs them.
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(server))
                .title("Devil's Rage")
                .inner_size(1400.0, 900.0)
                .min_inner_size(800.0, 500.0)
                .center()
                .build()?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Devil's Rage app");
}
