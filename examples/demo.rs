//! Try the update flow against a feed:
//!
//!     cargo run --example demo -- [--bundle[=<app-id>]] <appcast-url> <base64-public-key> [current-version] [appimage-to-replace] [icon.png]
//!
//! Without an AppImage path (and no `$APPIMAGE`), updates open in the browser.
//! `--bundle` turns on bundle mode, with an optional app id.

fn main() {
    linsparkle::run_if_helper();

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(flag) = args.first().filter(|a| *a == "--bundle" || a.starts_with("--bundle=")).cloned() {
        args.remove(0);
        linsparkle::set_bundle_mode(true);
        if let Some(id) = flag.strip_prefix("--bundle=") {
            linsparkle::set_bundle_app_id(id);
        }
    }
    let [url, key, rest @ ..] = args.as_slice() else {
        eprintln!(
            "usage: demo [--bundle[=<app-id>]] <appcast-url> <base64-public-key> [current-version] [appimage-to-replace] [icon.png]"
        );
        std::process::exit(2);
    };
    linsparkle::set_appcast_url(url);
    if !linsparkle::set_eddsa_public_key(key) {
        std::process::exit(2);
    }
    linsparkle::set_app_details("LinSparkle Demo", rest.first().map_or("1.0.0", String::as_str));
    if let Some(path) = rest.get(1).filter(|p| !p.is_empty()) {
        linsparkle::set_install_path(path);
    }
    if let Some(icon) = rest.get(2) {
        linsparkle::set_app_icon(icon);
    }
    linsparkle::set_shutdown_request_callback(|| {
        println!("demo: the updater asked us to quit; the new version starts once we exit");
        std::process::exit(0);
    });

    linsparkle::check_update_with_ui();
    while linsparkle::is_checking() {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    linsparkle::cleanup();
}
