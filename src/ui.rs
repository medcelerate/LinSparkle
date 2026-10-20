//! The helper process: checks the feed and walks through the update in a
//! small window of its own, like Sparkle's and WinSparkle's.

use crate::appcast::{self, Item};
use crate::{Config, SHUTDOWN_LINE, install, prefs};
use slint::ComponentHandle;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// Allowed: the markup keeps ScrollView's viewport-* properties, which Slint
// 1.18 deprecated, so it still builds against Slint 1.17.
#[allow(deprecated)]
mod window {
    slint::slint! {
        import { Button, ProgressIndicator, ScrollView, Palette } from "std-widgets.slint";

        component Heading inherits Text {
            font-size: 15px;
            font-weight: 700;
            wrap: word-wrap;
        }

        component Body inherits Text {
            wrap: word-wrap;
        }

        // The app's icon, or a stand-in when there isn't one.
        component AppIcon inherits Rectangle {
            in property <image> icon;
            in property <bool> has-icon;
            width: 64px;
            height: 64px;
            if root.has-icon: Image {
                width: parent.width;
                height: parent.height;
                source: root.icon;
                image-fit: contain;
            }
            if !root.has-icon: Rectangle {
                border-radius: 14px;
                // Solid: the software renderer drops the radius on gradients.
                background: #2f80e4;
                Text {
                    text: "↓";
                    color: white;
                    font-size: 36px;
                    font-weight: 700;
                }
            }
        }

        // Release notes in a sunken panel, like WinSparkle's.
        component NotesPanel inherits Rectangle {
            in property <string> text;
            background: Palette.alternate-background;
            border-width: 1px;
            border-color: Palette.border;
            border-radius: 4px;
            clip: true;
            notes-view := ScrollView {
                // viewport-*: content-* only exists from Slint 1.18.
                viewport-width: self.visible-width;
                viewport-height: notes-text.preferred-height + 24px;
                notes-text := Text {
                    x: 12px;
                    y: 12px;
                    width: notes-view.visible-width - 24px;
                    text: root.text == "" ? "No release notes." : root.text;
                    wrap: word-wrap;
                }
            }
        }

        export component UpdateWindow inherits Window {
            in property <int> page; // see the PAGE_ constants
            in property <string> app-name;
            in property <string> current-version;
            in property <string> new-version;
            in property <string> notes;
            in property <string> error-text;
            in property <string> progress-text;
            in property <float> progress;
            in property <bool> can-install;
            in property <bool> can-relaunch;
            in property <bool> has-link;
            in property <image> app-icon;
            in property <bool> has-icon;
            // Bundle mode: the other apps this update also replaces.
            in property <string> also-updates;

            callback dismiss();
            callback skip();
            callback install();
            callback cancel();
            callback install-now();
            callback open-link();

            title: root.app-name + " Update";
            icon: root.app-icon;

            // WinSparkle's layout: the icon in a column of its own, everything
            // else (text, notes, buttons) inset to its right.
            HorizontalLayout {
                padding: 20px;
                spacing: 18px;

                VerticalLayout {
                    alignment: start;
                    AppIcon { icon: root.app-icon; has-icon: root.has-icon; }
                }

                if root.page == 0: VerticalLayout {
                    spacing: 12px;
                    Heading { text: "Checking for updates…"; }
                    ProgressIndicator { indeterminate: true; height: 6px; }
                    Rectangle { vertical-stretch: 1; }
                    HorizontalLayout {
                        alignment: end;
                        Button { text: "Cancel"; clicked => { root.dismiss(); } }
                    }
                }

                if root.page == 1: VerticalLayout {
                    spacing: 10px;
                    Heading { text: "You’re up to date!"; }
                    Body { text: root.app-name + " " + root.current-version + " is currently the newest version available."; }
                    Rectangle { vertical-stretch: 1; }
                    HorizontalLayout {
                        alignment: end;
                        Button { text: "OK"; primary: true; clicked => { root.dismiss(); } }
                    }
                }

                if root.page == 2: VerticalLayout {
                    spacing: 10px;
                    Heading { text: "A new version of " + root.app-name + " is available!"; }
                    Body {
                        text: root.app-name + " " + root.new-version + " is now available — you have " + root.current-version + ". Would you like to " + (root.can-install ? "install" : "download") + " it now?";
                    }
                    Text { text: "Release Notes:"; font-weight: 700; }
                    NotesPanel {
                        text: root.notes;
                        vertical-stretch: 1;
                        min-height: 140px;
                    }
                    HorizontalLayout {
                        spacing: 8px;
                        padding-top: 6px;
                        Button { text: "Skip This Version"; clicked => { root.skip(); } }
                        Rectangle { horizontal-stretch: 1; }
                        Button { text: "Remind Me Later"; clicked => { root.dismiss(); } }
                        Button {
                            text: root.can-install ? "Install Update" : "Download…";
                            primary: true;
                            clicked => { root.install(); }
                        }
                    }
                }

                if root.page == 3: VerticalLayout {
                    spacing: 10px;
                    Heading { text: "Downloading " + root.app-name + " " + root.new-version + "…"; }
                    ProgressIndicator { progress: root.progress; height: 6px; }
                    Rectangle { vertical-stretch: 1; }
                    HorizontalLayout {
                        Body { text: root.progress-text; vertical-alignment: center; }
                        Button { text: "Cancel"; clicked => { root.cancel(); } }
                    }
                }

                if root.page == 4: VerticalLayout {
                    spacing: 10px;
                    Heading { text: "Ready to install"; }
                    Body {
                        text: root.app-name + " " + root.new-version + " has been downloaded and verified." + (root.also-updates != "" ? " It also updates " + root.also-updates + "." : "") + (root.can-relaunch ? " " + root.app-name + " will quit and reopen to finish installing." : "");
                    }
                    Rectangle { vertical-stretch: 1; }
                    HorizontalLayout {
                        spacing: 8px;
                        alignment: end;
                        Button { text: "Cancel"; clicked => { root.cancel(); } }
                        Button {
                            text: root.can-relaunch ? "Install and Relaunch" : "Install";
                            primary: true;
                            clicked => { root.install-now(); }
                        }
                    }
                }

                if root.page == 5: VerticalLayout {
                    spacing: 10px;
                    Heading { text: "Update error"; }
                    Body { text: root.error-text; }
                    Rectangle { vertical-stretch: 1; }
                    HorizontalLayout {
                        spacing: 8px;
                        alignment: end;
                        if root.has-link: Button { text: "Open Download Page"; clicked => { root.open-link(); } }
                        Button { text: "OK"; primary: true; clicked => { root.dismiss(); } }
                    }
                }

                if root.page == 6: VerticalLayout {
                    spacing: 10px;
                    Heading { text: "Update installed"; }
                    Body { text: root.app-name + " " + root.new-version + " will start the next time you open " + root.app-name + "."; }
                    Rectangle { vertical-stretch: 1; }
                    HorizontalLayout {
                        alignment: end;
                        Button { text: "OK"; primary: true; clicked => { root.dismiss(); } }
                    }
                }
            }
        }
    }
}
use window::UpdateWindow;

const PAGE_CHECKING: i32 = 0;
const PAGE_UP_TO_DATE: i32 = 1;
const PAGE_AVAILABLE: i32 = 2;
const PAGE_DOWNLOADING: i32 = 3;
const PAGE_READY: i32 = 4;
const PAGE_ERROR: i32 = 5;
const PAGE_INSTALLED: i32 = 6;

#[derive(Default)]
struct Session {
    item: Option<Item>,
    staged: Option<install::Staged>,
}

fn lock(s: &Mutex<Session>) -> std::sync::MutexGuard<'_, Session> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn show_page(w: &UpdateWindow, page: i32) {
    w.set_page(page);
    let (width, height) = match page {
        PAGE_AVAILABLE => (680.0, 500.0),
        PAGE_ERROR => (540.0, 210.0),
        PAGE_READY if !w.get_also_updates().is_empty() => (540.0, 200.0),
        _ => (540.0, 170.0),
    };
    w.window().set_size(slint::LogicalSize::new(width, height));
}

fn show_error(w: &UpdateWindow, message: &str, has_link: bool) {
    let mut text = message.to_string();
    if let Some(first) = text.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    if !text.ends_with('.') {
        text.push('.');
    }
    w.set_error_text(text.into());
    w.set_has_link(has_link);
    show_page(w, PAGE_ERROR);
    let _ = w.show();
}

/// The window's icon: the one the app set, else the AppImage's.
fn load_icon(cfg: &Config) -> Option<slint::Image> {
    let path = cfg.icon_path.clone().or_else(|| {
        std::env::var_os("APPDIR")
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join(".DirIcon"))
    })?;
    match load_png(&path) {
        Ok(image) => Some(image),
        Err(e) => {
            eprintln!("linsparkle: couldn't load the icon {}: {e}", path.display());
            None
        }
    }
}

/// Decode a PNG by content, not extension: `.DirIcon` has none.
fn load_png(path: &std::path::Path) -> Result<slint::Image, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("the image is too large")?];
    let frame = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let pixels = &buf[..frame.buffer_size()];
    let rgba: Vec<u8> = match frame.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect(),
        png::ColorType::GrayscaleAlpha => pixels.as_chunks::<2>().0.iter().flat_map(|&[g, a]| [g, g, g, a]).collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("unexpected indexed colour after expansion".into()),
    };
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, frame.width, frame.height);
    Ok(slint::Image::from_rgba8(buffer))
}

fn quit() {
    let _ = slint::quit_event_loop();
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// "A", "A and B", "A, B and C": the other apps a bundle updates, by the
/// names `Staged::others` gives them.
fn list_apps(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Run the helper; returns the process exit code.
pub fn run(cfg: Config, interactive: bool, host_pid: u32, can_relaunch: bool) -> i32 {
    // The helper is the host app's own executable, so Slint's default pick
    // is among whatever renderers the app compiled in: Skia or FemtoVG over
    // OpenGL, say, which in an app that sets its renderer up itself can show
    // this window mostly unpainted. The software renderer is the one
    // LinSparkle brings, so use it.
    if let Err(e) = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()
    {
        eprintln!("linsparkle: couldn't select the software renderer, using the default: {e}");
    }
    let window = match UpdateWindow::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("linsparkle: couldn't open the update window: {e}");
            return 1;
        }
    };
    let app_name = if cfg.app_name.is_empty() { "This app".to_string() } else { cfg.app_name.clone() };
    // In bundle mode, also only when the directory takes new files: the
    // other apps' updates are staged there too. Otherwise, as before, the
    // update is offered for download.
    let target = cfg
        .install_path
        .clone()
        .filter(|p| p.is_file())
        .filter(|p| !cfg.bundle || install::dir_writable(p));
    let bundle = cfg.bundle.then(|| install::BundleOptions { app_id: cfg.bundle_app_id.clone() });
    let cfg = Arc::new(cfg);
    let session = Arc::new(Mutex::new(Session::default()));
    let cancel = Arc::new(AtomicBool::new(false));

    window.set_app_name(app_name.clone().into());
    window.set_current_version(cfg.app_version.clone().into());
    window.set_can_install(target.is_some());
    window.set_can_relaunch(can_relaunch && host_pid != 0);
    if let Some(icon) = load_icon(&cfg) {
        window.set_app_icon(icon);
        window.set_has_icon(true);
    }

    // Leaving at any point: stop a download, drop a staged update.
    let leave = {
        let session = session.clone();
        let cancel = cancel.clone();
        move || {
            cancel.store(true, Ordering::Relaxed);
            if let Some(staged) = lock(&session).staged.take() {
                install::discard(staged);
            }
            quit();
        }
    };
    window.on_dismiss(leave.clone());
    window.on_cancel(leave.clone());
    window.window().on_close_requested({
        let leave = leave.clone();
        move || {
            leave();
            slint::CloseRequestResponse::HideWindow
        }
    });

    window.on_skip({
        let session = session.clone();
        let cfg = cfg.clone();
        move || {
            if let Some(item) = &lock(&session).item {
                prefs::set_skipped_version(&cfg.app_name, &item.version);
            }
            quit();
        }
    });

    window.on_open_link({
        let session = session.clone();
        move || {
            if let Some(item) = &lock(&session).item {
                install::open_url(item.link.as_deref().unwrap_or(&item.url));
            }
            quit();
        }
    });

    window.on_install({
        let weak = window.as_weak();
        let session = session.clone();
        let cfg = cfg.clone();
        let cancel = cancel.clone();
        let target = target.clone();
        move || {
            let Some(item) = lock(&session).item.clone() else { return };
            let Some(target) = target.clone() else {
                // Not an AppImage we can replace: hand over to the browser.
                install::open_url(item.link.as_deref().unwrap_or(&item.url));
                quit();
                return;
            };
            if let Some(w) = weak.upgrade() {
                w.set_progress(0.0);
                w.set_progress_text("Connecting…".into());
                show_page(&w, PAGE_DOWNLOADING);
            }
            let weak = weak.clone();
            let session = session.clone();
            let cfg = cfg.clone();
            let cancel = cancel.clone();
            let bundle = bundle.clone();
            std::thread::spawn(move || {
                let agent = install::agent(&cfg);
                let mut last = Instant::now() - Duration::from_secs(1);
                let progress = |done: u64, total: Option<u64>| {
                    if last.elapsed() < Duration::from_millis(100) && total != Some(done) {
                        return;
                    }
                    last = Instant::now();
                    let (fraction, text) = match total {
                        Some(t) if t > 0 => (done as f32 / t as f32, format!("{} of {}", megabytes(done), megabytes(t))),
                        _ => (0.0, megabytes(done)),
                    };
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        w.set_progress(fraction);
                        w.set_progress_text(text.into());
                    });
                };
                let result = install::download(&agent, &cfg, &item, &cancel, progress)
                    .and_then(|data| install::stage(&target, &data, bundle.as_ref()));
                if cancel.load(Ordering::Relaxed) {
                    if let Ok(staged) = result {
                        install::discard(staged);
                    }
                    return;
                }
                let has_link = item.link.is_some();
                match result {
                    Ok(staged) => {
                        let others = list_apps(&staged.others());
                        lock(&session).staged = Some(staged);
                        let _ = weak.upgrade_in_event_loop(move |w| {
                            w.set_also_updates(others.into());
                            show_page(&w, PAGE_READY);
                        });
                    }
                    Err(e) => {
                        eprintln!("linsparkle: {e}");
                        let _ = weak.upgrade_in_event_loop(move |w| show_error(&w, &e, has_link));
                    }
                }
            });
        }
    });

    window.on_install_now({
        let weak = window.as_weak();
        let session = session.clone();
        let target = target.clone();
        move || {
            let (Some(w), Some(target)) = (weak.upgrade(), target.clone()) else { return };
            let (staged, has_link) = {
                let mut s = lock(&session);
                (s.staged.take(), s.item.as_ref().is_some_and(|i| i.link.is_some()))
            };
            let Some(staged) = staged else { return };
            if let Err(e) = install::install(staged) {
                eprintln!("linsparkle: {e}");
                show_error(&w, &e, has_link);
                return;
            }
            if !w.get_can_relaunch() {
                show_page(&w, PAGE_INSTALLED);
                return;
            }
            match install::relaunch_after_exit(host_pid, &target) {
                Ok(()) => {
                    let mut out = std::io::stdout();
                    let _ = writeln!(out, "{SHUTDOWN_LINE}").and_then(|_| out.flush());
                    quit();
                }
                Err(e) => {
                    eprintln!("linsparkle: {e}");
                    show_page(&w, PAGE_INSTALLED);
                }
            }
        }
    });

    if interactive {
        show_page(&window, PAGE_CHECKING);
        if let Err(e) = window.show() {
            eprintln!("linsparkle: couldn't show the update window: {e}");
            return 1;
        }
    }

    // The check itself, off the UI thread.
    {
        let weak = window.as_weak();
        let session = session.clone();
        let cfg = cfg.clone();
        std::thread::spawn(move || {
            let agent = install::agent(&cfg);
            let items = match install::fetch_appcast(&agent, &cfg) {
                Ok(items) => items,
                Err(e) => {
                    eprintln!("linsparkle: {e}");
                    if interactive {
                        let _ = weak.upgrade_in_event_loop(move |w| show_error(&w, &e, false));
                    } else {
                        let _ = slint::invoke_from_event_loop(quit);
                    }
                    return;
                }
            };
            prefs::record_check(&cfg.app_name);
            let skipped = if interactive { None } else { prefs::skipped_version(&cfg.app_name) };
            let update = appcast::newest_update(items, &cfg.app_version)
                .filter(|item| skipped.as_deref() != Some(item.version.as_str()));
            let Some(item) = update else {
                if interactive {
                    let _ = weak.upgrade_in_event_loop(|w| show_page(&w, PAGE_UP_TO_DATE));
                } else {
                    let _ = slint::invoke_from_event_loop(quit);
                }
                return;
            };
            let notes = install::fetch_notes(&agent, &item);
            let version = item.display_version.clone();
            let has_link = item.link.is_some();
            lock(&session).item = Some(item);
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_new_version(version.into());
                w.set_notes(notes.into());
                w.set_has_link(has_link);
                show_page(&w, PAGE_AVAILABLE);
                let _ = w.show();
            });
        });
    }

    if let Err(e) = slint::run_event_loop_until_quit() {
        eprintln!("linsparkle: {e}");
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::list_apps;
    use crate::bundle::{self, tests::bundle as bundle_tgz, tests::elf};
    use crate::install;

    #[test]
    fn lists_apps() {
        let l = |v: &[&str]| list_apps(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(l(&[]), "");
        assert_eq!(l(&["Show Alchemy"]), "Show Alchemy");
        assert_eq!(l(&["Show Alchemy", "Trace Alchemy"]), "Show Alchemy and Trace Alchemy");
        assert_eq!(l(&["A", "B", "C"]), "A, B and C");
    }

    #[test]
    fn lists_apps_by_their_manifest_names() {
        let (l, s, t, x) = (elf("loop"), elf("show"), elf("trace"), elf("extra"));
        let dir = std::env::temp_dir().join(format!("linsparkle-ui-names-{}", std::process::id()));
        let (loop_file, show_file, trace_file) = (
            "Excalibur_Loop_Alchemy-x86_64.AppImage",
            "Excalibur_Show_Alchemy-x86_64.AppImage",
            "Excalibur_Trace_Alchemy-x86_64.AppImage",
        );
        // What the ready page says the update also replaces, run from Loop
        // with Show, Trace and a helper installed beside it.
        let also_updates = |manifest: Option<&[u8]>| {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            for f in [loop_file, show_file, trace_file, "extra-tool"] {
                std::fs::write(dir.join(f), b"old").unwrap();
            }
            let mut files: Vec<(&str, &[u8])> =
                vec![(loop_file, &l), (show_file, &s), (trace_file, &t), ("extra-tool", &x)];
            if let Some(m) = manifest {
                files.push((bundle::MANIFEST, m));
            }
            let opts = install::BundleOptions::default();
            let staged = install::stage(&dir.join(loop_file), &bundle_tgz(&files), Some(&opts)).unwrap();
            let text = list_apps(&staged.others());
            install::discard(staged);
            text
        };
        // Named, unnamed (its file name less .AppImage) and a member that
        // isn't an AppImage; in the order of their files, as staged.
        assert_eq!(
            also_updates(Some(
                br#"{"members": [
                {"file": "Excalibur_Loop_Alchemy-x86_64.AppImage", "id": "loop", "name": "Loop Alchemy"},
                {"file": "Excalibur_Show_Alchemy-x86_64.AppImage", "name": "Show Alchemy"},
                {"file": "Excalibur_Trace_Alchemy-x86_64.AppImage", "name": "  Trace Alchemy "},
                {"file": "extra-tool"}]}"#
            )),
            "Show Alchemy, Trace Alchemy and extra-tool"
        );
        assert_eq!(
            also_updates(Some(
                br#"{"members": [
                {"file": "Excalibur_Loop_Alchemy-x86_64.AppImage"},
                {"file": "Excalibur_Show_Alchemy-x86_64.AppImage", "name": "Show Alchemy"},
                {"file": "Excalibur_Trace_Alchemy-x86_64.AppImage", "name": null}]}"#
            )),
            "Show Alchemy and Excalibur_Trace_Alchemy-x86_64"
        );
        // No manifest: file names, as before names.
        assert_eq!(also_updates(None), "Excalibur_Show_Alchemy-x86_64, Excalibur_Trace_Alchemy-x86_64 and extra-tool");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
