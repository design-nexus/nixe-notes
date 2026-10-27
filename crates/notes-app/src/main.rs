mod prefs;
mod window;
mod worker;
use gtk::{gio, glib, prelude::*};
fn main() -> glib::ExitCode {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "Notes — native Markdown workspace\nUsage: nyx-notes [--vault DIRECTORY] [FILE]\n       nyx-notes --version"
        );
        return glib::ExitCode::SUCCESS;
    }
    if std::env::args().any(|a| a == "--version") {
        println!("nyx-notes {}", env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }
    nyx_core::app::init(nyx_core::app::App {
        slug: "nyx-notes",
        name: "Notes",
    });
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: environment setup precedes all worker threads.
        unsafe { std::env::set_var("GSK_RENDERER", "ngl") };
    }
    let app = gtk::Application::builder()
        .application_id("io.github.design_nexus.nyx.Notes")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_startup(|app| {
        nyx_core::control::serve_async(app, window::integration_tools(), window::integration_call)
    });
    app.connect_command_line(|app, line| {
        let args = line.arguments();
        let mut vault = None;
        let mut files = Vec::new();
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--launcher-action" {
                i += 1;
                let Some(value) = args.get(i) else {
                    eprintln!("--launcher-action needs JSON");
                    return 2.into();
                };
                let result = serde_json::from_str::<notes_vault::integrations::LauncherAction>(
                    &value.to_string_lossy(),
                )
                .map_err(|e| e.to_string())
                .and_then(|request| window::launcher_action(app, request));
                return match result {
                    Ok(()) => glib::ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("Notes launcher: {e}");
                        2.into()
                    }
                };
            } else if args[i] == "--vault" {
                i += 1;
                let Some(value) = args.get(i) else {
                    eprintln!("--vault needs a directory");
                    return 2.into();
                };
                vault = line.create_file_for_arg(value).path();
            } else if args[i].to_string_lossy().starts_with('-') {
                eprintln!("Unknown option; use --help");
                return 2.into();
            } else if let Some(path) = line.create_file_for_arg(&args[i]).path() {
                files.push(path);
            }
            i += 1;
        }
        window::present(app, vault, files);
        glib::ExitCode::SUCCESS
    });
    app.connect_shutdown(|_| prefs::flush());
    app.run()
}
