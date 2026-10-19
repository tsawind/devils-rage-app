use tauri::{Url, WebviewUrl, WebviewWindowBuilder};

mod settings;
#[cfg(desktop)]
mod updates;

/// The live mapper. Phase 0 step 3 turns this into a setting (live / test).
pub const DEFAULT_SERVER: &str = "https://devilsrage.duckdns.org";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_process::init());
    }

    builder
        .setup(|app| {
            let server: Url = DEFAULT_SERVER.parse()?;

            // Stable builds show just the name; test builds show which build it
            // is, so it's obvious when an update has landed.
            let version = app.package_info().version.to_string();
            let channel = settings::load(app.handle()).channel(app.handle());
            let title = match channel {
                settings::Channel::Stable => "Devil's Rage".to_string(),
                _ => format!("Devil's Rage — {version} ({})", channel.as_str()),
            };

            // The main window is created in code (not in tauri.conf.json) so the
            // server address can come from settings later without a rebuild.
            // The mapper page is remote content: it gets NO access to the app's
            // native commands. Native features will be exposed deliberately, per
            // command, when Phase 1 needs them.
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(server))
                .title(title)
                .inner_size(1400.0, 900.0)
                .min_inner_size(800.0, 500.0)
                .center()
                .build()?;

            #[cfg(desktop)]
            updates::start(app.handle().clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Devil's Rage app");
}
