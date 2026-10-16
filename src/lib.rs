//! LinSparkle: Sparkle-style updates for Linux AppImages.
//!
//! It reads the same appcast and Ed25519 signatures as Sparkle (macOS) and
//! WinSparkle (Windows), so one release pipeline feeds all three, and its
//! calls mirror WinSparkle's:
//!
//! ```no_run
//! fn main() {
//!     // First thing: LinSparkle's UI runs as this same executable in a
//!     // helper process, and this is where that process branches off.
//!     linsparkle::run_if_helper();
//!
//!     linsparkle::set_appcast_url("https://example.com/appcast/linux.xml");
//!     linsparkle::set_eddsa_public_key("<base64 Ed25519 public key>");
//!     linsparkle::set_app_details("My App", env!("CARGO_PKG_VERSION"));
//!     linsparkle::set_shutdown_request_callback(|| { /* quit the app */ });
//!     linsparkle::init();
//!     // ... a "Check for Updates…" menu item calls:
//!     linsparkle::check_update_with_ui();
//!     // ... and on exit:
//!     linsparkle::cleanup();
//! }
//! ```
//!
//! The UI is a separate process so it never touches the app's own event
//! loop or toolkit. When someone installs an update, the helper verifies it,
//! swaps it in over the running AppImage (`$APPIMAGE`), arranges a relaunch,
//! and calls the shutdown callback for the app to quit.

mod appcast;
mod install;
mod notes;
mod prefs;
mod ui;
mod verify;
pub mod version;

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

pub use appcast::Item;

const ENV_MODE: &str = "LINSPARKLE_HELPER";
const ENV_APPCAST: &str = "LINSPARKLE_APPCAST_URL";
const ENV_PUBKEY: &str = "LINSPARKLE_PUBLIC_KEY";
const ENV_APP_NAME: &str = "LINSPARKLE_APP_NAME";
const ENV_APP_VERSION: &str = "LINSPARKLE_APP_VERSION";
const ENV_INSTALL_PATH: &str = "LINSPARKLE_INSTALL_PATH";
const ENV_HOST_PID: &str = "LINSPARKLE_HOST_PID";
const ENV_CAN_RELAUNCH: &str = "LINSPARKLE_CAN_RELAUNCH";
const ENV_ICON: &str = "LINSPARKLE_ICON";
const ALL_ENV: &[&str] = &[
    ENV_MODE,
    ENV_APPCAST,
    ENV_PUBKEY,
    ENV_APP_NAME,
    ENV_APP_VERSION,
    ENV_INSTALL_PATH,
    ENV_HOST_PID,
    ENV_CAN_RELAUNCH,
    ENV_ICON,
];
/// What the helper prints on stdout when the app should quit to finish an
/// update.
const SHUTDOWN_LINE: &str = "linsparkle:shutdown";

/// WinSparkle's floor on the automatic check interval.
const MIN_CHECK_INTERVAL: u64 = 3600;

#[derive(Clone, Debug, Default)]
pub(crate) struct Config {
    pub appcast_url: String,
    pub public_key: String,
    pub app_name: String,
    pub app_version: String,
    /// The AppImage to replace; `None` when not running as one, in which
    /// case updates can only be downloaded by hand.
    pub install_path: Option<PathBuf>,
    /// A PNG for the update window; else the AppImage's `.DirIcon`.
    pub icon_path: Option<PathBuf>,
}

type Callback = Arc<dyn Fn() + Send + Sync>;

struct State {
    config: Config,
    install_path_set: bool,
    automatic: bool,
    interval: u64,
    shutdown: Option<Callback>,
    helper: Option<Child>,
}

static STATE: Mutex<State> = Mutex::new(State {
    config: Config {
        appcast_url: String::new(),
        public_key: String::new(),
        app_name: String::new(),
        app_version: String::new(),
        install_path: None,
        icon_path: None,
    },
    install_path_set: false,
    automatic: false,
    interval: 86400,
    shutdown: None,
    helper: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The feed to check: a Sparkle appcast. Items whose enclosure has a
/// `sparkle:os` other than `linux` are ignored.
pub fn set_appcast_url(url: &str) {
    state().config.appcast_url = url.to_string();
}

/// The base64 Ed25519 public key (Sparkle's `SUPublicEDKey`). Every update
/// must carry a matching `sparkle:edSignature`. Returns false, and keeps
/// the old key, if this one isn't valid.
pub fn set_eddsa_public_key(base64_key: &str) -> bool {
    match verify::public_key(base64_key) {
        Ok(_) => {
            state().config.public_key = base64_key.trim().to_string();
            true
        }
        Err(e) => {
            eprintln!("linsparkle: {e}");
            false
        }
    }
}

/// The app's name as people know it, and its version as the appcast's
/// `sparkle:version` numbers it.
pub fn set_app_details(name: &str, version: &str) {
    let mut s = state();
    s.config.app_name = name.to_string();
    s.config.app_version = version.to_string();
}

/// The AppImage to update in place. Defaults to `$APPIMAGE`, which the
/// AppImage runtime sets; without either, updates open in the browser.
pub fn set_install_path(path: impl Into<PathBuf>) {
    let mut s = state();
    s.config.install_path = Some(path.into());
    s.install_path_set = true;
}

/// The icon (a PNG) for the update window. Inside an AppImage it defaults
/// to the AppImage's own icon (`$APPDIR/.DirIcon`).
pub fn set_app_icon(path: impl Into<PathBuf>) {
    state().config.icon_path = Some(path.into());
}

/// Check by itself, from `init()`, once every `set_update_check_interval`.
pub fn set_automatic_check_for_updates(enabled: bool) {
    state().automatic = enabled;
}

/// Seconds between automatic checks; at least an hour. Default: a day.
pub fn set_update_check_interval(seconds: u64) {
    state().interval = seconds.max(MIN_CHECK_INTERVAL);
}

/// Called (on a LinSparkle thread) when someone chooses to install an
/// update: the new version is in place and will start once this process
/// exits, so the app should quit. Without a callback the update is still
/// installed, and starts the next time the app is opened.
pub fn set_shutdown_request_callback(f: impl Fn() + Send + Sync + 'static) {
    state().shutdown = Some(Arc::new(f));
}

/// Call first thing in `main`: when this process is LinSparkle's helper,
/// runs the update UI and exits; otherwise returns at once.
pub fn run_if_helper() {
    let Ok(mode) = std::env::var(ENV_MODE) else { return };
    let env = |k| std::env::var(k).unwrap_or_default();
    let cfg = Config {
        appcast_url: env(ENV_APPCAST),
        public_key: env(ENV_PUBKEY),
        app_name: env(ENV_APP_NAME),
        app_version: env(ENV_APP_VERSION),
        install_path: std::env::var_os(ENV_INSTALL_PATH)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
        icon_path: std::env::var_os(ENV_ICON).filter(|p| !p.is_empty()).map(PathBuf::from),
    };
    let interactive = mode == "interactive";
    let host_pid = env(ENV_HOST_PID).parse().unwrap_or(0);
    let can_relaunch = env(ENV_CAN_RELAUNCH) == "1";
    let code = ui::run(cfg, interactive, host_pid, can_relaunch);
    std::process::exit(code);
}

/// Start LinSparkle: runs an automatic check if they're on and one is due.
pub fn init() {
    let (automatic, interval, app_name) = {
        let s = state();
        (s.automatic, s.interval, s.config.app_name.clone())
    };
    if automatic && prefs::since_last_check(&app_name).is_none_or(|since| since >= interval) {
        check_update_without_ui();
    }
}

/// Check now and show the result, including "you're up to date" and
/// errors; offers skipped versions too. For a "Check for Updates…" item.
pub fn check_update_with_ui() {
    spawn_helper(true);
}

/// Check quietly; only shows anything when there's an update that hasn't
/// been skipped.
pub fn check_update_without_ui() {
    spawn_helper(false);
}

/// Whether the update UI (or a quiet check) is running.
pub fn is_checking() -> bool {
    let mut s = state();
    matches!(s.helper.as_mut().map(|c| c.try_wait()), Some(Ok(None)))
}

/// Stop any running check or update window. Call before the app exits.
pub fn cleanup() {
    if let Some(mut child) = state().helper.take() {
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

fn spawn_helper(interactive: bool) {
    let mut s = state();
    if let Some(child) = s.helper.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return;
        }
    }
    let cfg = &s.config;
    if cfg.appcast_url.is_empty() || cfg.public_key.is_empty() || cfg.app_version.is_empty() {
        eprintln!("linsparkle: set the appcast URL, public key and app version before checking");
        return;
    }
    let install_path = if s.install_path_set {
        cfg.install_path.clone()
    } else {
        std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()).map(PathBuf::from)
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("linsparkle: can't find this executable to start the updater: {e}");
            return;
        }
    };
    let mut cmd = Command::new(exe);
    cmd.env(ENV_MODE, if interactive { "interactive" } else { "background" })
        .env(ENV_APPCAST, &cfg.appcast_url)
        .env(ENV_PUBKEY, &cfg.public_key)
        .env(ENV_APP_NAME, &cfg.app_name)
        .env(ENV_APP_VERSION, &cfg.app_version)
        .env(ENV_INSTALL_PATH, install_path.unwrap_or_default())
        .env(ENV_HOST_PID, std::process::id().to_string())
        .env(ENV_CAN_RELAUNCH, if s.shutdown.is_some() { "1" } else { "0" })
        .env(ENV_ICON, cfg.icon_path.clone().unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("linsparkle: couldn't start the updater: {e}");
            return;
        }
    };
    if let Some(stdout) = child.stdout.take() {
        let shutdown = s.shutdown.clone();
        let spawned = std::thread::Builder::new().name("linsparkle".into()).spawn(move || {
            for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.trim() == SHUTDOWN_LINE {
                    if let Some(f) = &shutdown {
                        f();
                    }
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("linsparkle: couldn't watch the updater: {e}");
        }
    }
    s.helper = Some(child);
}

/// Environment for programs the helper starts (the relaunched app, the
/// browser): without LinSparkle's variables, so the new app doesn't think
/// it's a helper, and without the old AppImage mount on its search paths.
pub(crate) fn clean_env(cmd: &mut Command) {
    for k in ALL_ENV {
        cmd.env_remove(k);
    }
    let Some(appdir) = std::env::var_os("APPDIR").filter(|d| !d.is_empty()) else { return };
    let appdir = PathBuf::from(appdir);
    for var in ["PATH", "LD_LIBRARY_PATH", "XDG_DATA_DIRS"] {
        let Some(value) = std::env::var_os(var) else { continue };
        let kept: Vec<PathBuf> = std::env::split_paths(&value)
            .filter(|p| !p.starts_with(&appdir))
            .collect();
        match std::env::join_paths(kept) {
            Ok(v) if v.is_empty() && var != "PATH" => {
                cmd.env_remove(var);
            }
            Ok(v) => {
                cmd.env(var, v);
            }
            Err(_) => {}
        }
    }
}
