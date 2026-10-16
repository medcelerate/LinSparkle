//! Fetching the feed and the update, and putting the new AppImage in place.
//!
//! The update file (a bare `.AppImage`, or a `.tar.gz` holding one so the
//! executable bit survives the download) is verified against the appcast's
//! Ed25519 signature before anything is written, staged next to the running
//! AppImage, and renamed over it. Linux lets you replace a running
//! executable's directory entry: the old one keeps running from its inode.

use crate::appcast::Item;
use crate::{Config, notes, verify};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const ELF_MAGIC: &[u8] = b"\x7fELF";
const GZIP_MAGIC: &[u8] = b"\x1f\x8b";

pub fn agent(cfg: &Config) -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(format!(
            "{}/{} LinSparkle/{}",
            cfg.app_name.replace(' ', ""),
            cfg.app_version,
            env!("CARGO_PKG_VERSION")
        ))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build()
        .into()
}

fn get_text(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    agent
        .get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())
}

pub fn fetch_appcast(agent: &ureq::Agent, cfg: &Config) -> Result<Vec<Item>, String> {
    let xml = get_text(agent, &cfg.appcast_url).map_err(|e| format!("couldn't reach the update server: {e}"))?;
    crate::appcast::parse(&xml)
}

/// The item's release notes as plain text; empty if there are none or they
/// can't be fetched.
pub fn fetch_notes(agent: &ureq::Agent, item: &Item) -> String {
    if let Some(link) = &item.release_notes_link {
        match get_text(agent, link) {
            Ok(html) => return notes::html_to_text(&html),
            Err(e) => eprintln!("linsparkle: couldn't fetch release notes from {link}: {e}"),
        }
    }
    item.description.as_deref().map(notes::html_to_text).unwrap_or_default()
}

/// Download the update and check its signature. `progress(done, total)` is
/// called as bytes arrive; setting `cancel` stops it.
pub fn download(
    agent: &ureq::Agent,
    cfg: &Config,
    item: &Item,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    let key = verify::public_key(&cfg.public_key)?;
    let signature = item
        .ed_signature
        .as_deref()
        .ok_or("the update isn't signed (no sparkle:edSignature), so it can't be trusted")?;

    let mut resp = agent
        .get(&item.url)
        .call()
        .map_err(|e| format!("couldn't download the update: {e}"))?;
    let total = resp.body().content_length().or((item.length > 0).then_some(item.length));
    let mut reader = resp.body_mut().as_reader();
    let mut data = Vec::with_capacity(total.unwrap_or(0).min(1 << 30) as usize);
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("the download failed: {e}"))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        progress(data.len() as u64, total);
    }
    if item.length > 0 && data.len() as u64 != item.length {
        return Err(format!(
            "the download is {} bytes but the update feed says {}",
            data.len(),
            item.length
        ));
    }
    verify::check(&key, signature, &data)?;
    Ok(data)
}

fn staged_path(target: &Path) -> PathBuf {
    let name = target.file_name().map_or("app".into(), |n| n.to_string_lossy().into_owned());
    target.with_file_name(format!(".{name}.linsparkle-new"))
}

/// The AppImage inside a verified update: the update itself when it's a
/// bare AppImage, else the `.AppImage` (or only executable) in its tarball.
/// Only ELF files count, which also passes over the `._` AppleDouble
/// entries macOS `tar` adds.
fn appimage_bytes(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.starts_with(ELF_MAGIC) {
        return Ok(data.to_vec());
    }
    if !data.starts_with(GZIP_MAGIC) {
        return Err("the update is neither an AppImage nor a .tar.gz".into());
    }
    let read_err = |e: std::io::Error| format!("couldn't read the update archive: {e}");
    let mut files = Vec::new();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
    for entry in archive.entries().map_err(read_err)? {
        let mut entry = entry.map_err(read_err)?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let name = entry.path().map_err(read_err)?.to_string_lossy().into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(read_err)?;
        if bytes.starts_with(ELF_MAGIC) {
            files.push((name, bytes));
        }
    }
    let pick = match files.iter().position(|(n, _)| n.to_ascii_lowercase().ends_with(".appimage")) {
        Some(i) => i,
        None if files.len() == 1 => 0,
        None => return Err("the update archive doesn't contain an AppImage".into()),
    };
    Ok(files.swap_remove(pick).1)
}

/// Write the new AppImage next to `target`, executable, ready to rename.
pub fn stage(target: &Path, data: &[u8]) -> Result<PathBuf, String> {
    let bytes = appimage_bytes(data)?;
    let staged = staged_path(target);
    let dir = target.parent().unwrap_or(Path::new("."));
    let write = || -> std::io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .open(&staged)?;
        f.write_all(&bytes)?;
        f.sync_all()
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("couldn't write the update to {}: {e}", dir.display())
    })?;
    Ok(staged)
}

/// Move the staged AppImage over `target`, keeping the old one's
/// permissions if they were wider (e.g. group-executable).
pub fn install(staged: &Path, target: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(target) {
        let mode = meta.permissions().mode() | 0o755;
        let _ = std::fs::set_permissions(staged, std::fs::Permissions::from_mode(mode & 0o7777));
    }
    std::fs::rename(staged, target).map_err(|e| {
        let _ = std::fs::remove_file(staged);
        format!("couldn't replace {}: {e}", target.display())
    })?;
    if let Ok(dir) = std::fs::File::open(target.parent().unwrap_or(Path::new("."))) {
        let _ = dir.sync_all();
    }
    Ok(())
}

pub fn discard(staged: &Path) {
    let _ = std::fs::remove_file(staged);
}

/// Start `target` once process `pid` (the app being updated) has exited.
/// Runs from /bin/sh, not from this executable: when an AppImage exits its
/// mount goes away, and anything still running out of it with it.
pub fn relaunch_after_exit(pid: u32, target: &Path) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec "$2""#)
        .arg("linsparkle-relaunch")
        .arg(pid.to_string())
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    crate::clean_env(&mut cmd);
    cmd.spawn().map(drop).map_err(|e| format!("couldn't schedule the relaunch: {e}"))
}

/// Open a URL in the desktop's browser.
pub fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let mut cmd = std::process::Command::new(opener);
    cmd.arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    crate::clean_env(&mut cmd);
    if let Err(e) = cmd.spawn() {
        eprintln!("linsparkle: couldn't open {url}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_appimage() -> Vec<u8> {
        let mut v = ELF_MAGIC.to_vec();
        v.extend_from_slice(b"rest of the new version");
        v
    }

    fn tarball(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn finds_the_appimage() {
        let app = fake_appimage();
        assert_eq!(appimage_bytes(&app).unwrap(), app);
        let tgz = tarball(&[("README", b"hi"), ("App-1.2.AppImage", &app)]);
        assert_eq!(appimage_bytes(&tgz).unwrap(), app);
        let tgz = tarball(&[("._App.AppImage", b"\x00\x05\x16\x07"), ("App.AppImage", &app)]);
        assert_eq!(appimage_bytes(&tgz).unwrap(), app);
        let tgz = tarball(&[("App", &app)]);
        assert_eq!(appimage_bytes(&tgz).unwrap(), app);
        assert!(appimage_bytes(&tarball(&[("a", b"x"), ("b", b"y")])).is_err());
        assert!(appimage_bytes(&tarball(&[("App.AppImage", b"#!/bin/sh")])).is_err());
        assert!(appimage_bytes(b"<html>").is_err());
    }

    /// Serve `body` once at http://127.0.0.1:<port>/.
    fn serve_once(body: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = conn.read(&mut req);
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            conn.write_all(head.as_bytes()).unwrap();
            conn.write_all(&body).unwrap();
        });
        format!("http://{addr}/App.tar.gz")
    }

    #[test]
    fn downloads_and_checks_the_signature() {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD;
        use ed25519_dalek::{Signer, SigningKey};

        let key = SigningKey::from_bytes(&[7u8; 32]);
        let tgz = tarball(&[("App.AppImage", &fake_appimage())]);
        let cfg = Config {
            public_key: STANDARD.encode(key.verifying_key().as_bytes()),
            app_name: "Test App".into(),
            app_version: "1.0".into(),
            ..Default::default()
        };
        let agent = agent(&cfg);
        let cancel = AtomicBool::new(false);
        let item = |url: String, sig: &[u8]| Item {
            version: "1.1".into(),
            url,
            ed_signature: Some(STANDARD.encode(sig)),
            ..Default::default()
        };

        let good = key.sign(&tgz).to_bytes();
        let mut seen = 0;
        let data = download(&agent, &cfg, &item(serve_once(tgz.clone()), &good), &cancel, |done, total| {
            assert_eq!(total, Some(tgz.len() as u64));
            seen = done;
        })
        .unwrap();
        assert_eq!(data, tgz);
        assert_eq!(seen, tgz.len() as u64);

        // One flipped byte: refused.
        let mut tampered = tgz.clone();
        *tampered.last_mut().unwrap() ^= 1;
        let err = download(&agent, &cfg, &item(serve_once(tampered), &good), &cancel, |_, _| {}).unwrap_err();
        assert!(err.contains("signature doesn't match"), "{err}");

        // Unsigned: refused before downloading.
        let mut unsigned = item(String::new(), &good);
        unsigned.ed_signature = None;
        assert!(download(&agent, &cfg, &unsigned, &cancel, |_, _| {}).is_err());
    }

    #[test]
    fn stages_and_replaces() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("linsparkle-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("App.AppImage");
        std::fs::write(&target, b"old").unwrap();

        let app = fake_appimage();
        let staged = stage(&target, &tarball(&[("App.AppImage", &app)])).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        install(&staged, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), app);
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o111, 0o111);
        assert!(!staged.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
