// Without the command line, a window-only app: no console window on
// Windows.
#![cfg_attr(all(windows, not(feature = "cli")), windows_subsystem = "windows")]

#[cfg(feature = "cli")]
mod cli;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result: Result<(), Box<dyn std::error::Error>> = match args.first().map(String::as_str) {
        // The input helper is always there: polkit and its service run the
        // app with these arguments.
        #[cfg(all(unix, not(target_os = "macos")))]
        Some("input-helper") => input_helper(&args[1..]),
        // Started at login or after an update: straight to the tray.
        Some("--background") => sisyphus_client::gui::run(true).map_err(Into::into),
        #[cfg(feature = "cli")]
        Some(_) => cli::run(&args),
        _ => sisyphus_client::gui::run(false).map_err(Into::into),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// The Wayland input helper service: `serve --uid N` (what the service
/// runs), `install`, `restart` and `uninstall` (as root).
#[cfg(all(unix, not(target_os = "macos")))]
fn input_helper(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use sisyphus_client::input_helper;
    match args.first().map(String::as_str) {
        Some("serve") => {
            let uid = match args.get(1..3) {
                Some([flag, uid]) if flag == "--uid" => uid.parse()?,
                _ => return Err("usage: input-helper serve --uid <uid>".into()),
            };
            input_helper::serve(uid)?;
        }
        Some("install") => {
            input_helper::install()?;
            println!("installed and started {}", input_helper::SERVICE);
        }
        Some("restart") => {
            input_helper::restart()?;
            println!("restarted {}", input_helper::SERVICE);
        }
        Some("uninstall") => {
            input_helper::uninstall()?;
            println!("removed {}", input_helper::SERVICE);
        }
        _ => return Err("usage: input-helper install|restart|uninstall|serve --uid <uid>".into()),
    }
    Ok(())
}
