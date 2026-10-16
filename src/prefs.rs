//! What LinSparkle remembers per app: the version someone chose to skip and
//! when it last checked. Plain files under `$XDG_CONFIG_HOME/linsparkle/<app>`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn dir(app_name: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let app: String = app_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    Some(base.join("linsparkle").join(if app.is_empty() { "app".into() } else { app }))
}

fn read(app_name: &str, file: &str) -> Option<String> {
    let s = std::fs::read_to_string(dir(app_name)?.join(file)).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn write(app_name: &str, file: &str, value: &str) {
    let Some(dir) = dir(app_name) else { return };
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(file), value)) {
        eprintln!("linsparkle: couldn't save {file}: {e}");
    }
}

pub fn skipped_version(app_name: &str) -> Option<String> {
    read(app_name, "skipped_version")
}

pub fn set_skipped_version(app_name: &str, version: &str) {
    write(app_name, "skipped_version", version);
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Seconds since the last check, `None` if it never checked.
pub fn since_last_check(app_name: &str) -> Option<u64> {
    let last: u64 = read(app_name, "last_check")?.parse().ok()?;
    Some(now().saturating_sub(last))
}

pub fn record_check(app_name: &str) {
    write(app_name, "last_check", &now().to_string());
}
