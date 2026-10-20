//! Fetching the feed and the update, and putting the new AppImage in place.
//!
//! The update file (a bare `.AppImage`, or a `.tar.gz` holding one so the
//! executable bit survives the download) is verified against the appcast's
//! Ed25519 signature before anything is written, staged next to the running
//! AppImage, and renamed over it. Linux lets you replace a running
//! executable's directory entry: the old one keeps running from its inode.
//! In bundle mode (see `bundle`) one update also replaces the other apps of
//! a suite installed beside it.

use crate::appcast::Item;
use crate::{Config, bundle, notes, verify};
use std::io::Read;
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
/// bare AppImage, else the one in its tarball: the one named like `target`,
/// else the first `.AppImage`, else the only executable. Only ELF files
/// count, which also passes over the `._` AppleDouble entries macOS `tar`
/// adds.
fn appimage_bytes(data: &[u8], target: &Path) -> Result<Vec<u8>, String> {
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
    let own_name = target.file_name().map(|n| n.to_string_lossy().into_owned());
    let base = |n: &str| n.rsplit('/').next().unwrap_or(n).to_string();
    let pick = match files.iter().position(|(n, _)| Some(base(n)) == own_name) {
        Some(i) => i,
        None => match files.iter().position(|(n, _)| n.to_ascii_lowercase().ends_with(".appimage")) {
            Some(i) => i,
            None if files.len() == 1 => 0,
            None => return Err("the update archive doesn't contain an AppImage".into()),
        },
    };
    Ok(files.swap_remove(pick).1)
}

/// Bundle mode's settings, from `set_bundle_mode` and `set_bundle_app_id`.
#[derive(Clone, Debug, Default)]
pub struct BundleOptions {
    pub app_id: Option<String>,
}

/// One staged file and what it replaces.
#[derive(Debug)]
struct StagedFile {
    staged: PathBuf,
    target: PathBuf,
    /// What to call it: the manifest's display name, else the file name
    /// less `.AppImage`.
    label: String,
}

/// An update written next to its targets, ready to rename over them. The
/// first file replaces the running AppImage.
#[derive(Debug)]
pub struct Staged {
    files: Vec<StagedFile>,
}

impl Staged {
    /// The names of the other apps this update replaces, as the window
    /// shows them: each one's display name from the bundle's manifest, else
    /// its file name less `.AppImage`.
    pub fn others(&self) -> Vec<String> {
        self.files[1..].iter().map(|f| f.label.clone()).collect()
    }
}

/// Write one staged file, executable; on failure leave nothing behind.
fn write_staged(staged: &Path, contents: &mut dyn Read) -> Result<(), String> {
    let dir = staged.parent().unwrap_or(Path::new("."));
    // A leftover from an update that didn't finish.
    let _ = std::fs::remove_file(staged);
    let mut write = || -> std::io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .custom_flags(libc::O_NOFOLLOW)
            .open(staged)?;
        std::io::copy(contents, &mut f)?;
        f.sync_all()
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(staged);
        format!("couldn't write the update to {}: {e}", dir.display())
    })
}

/// Write the new AppImage next to `target`, executable, ready to rename.
/// In bundle mode a `.tar.gz` update is a bundle (see `bundle`): its other
/// members are staged next to the files they replace, all or none.
pub fn stage(target: &Path, data: &[u8], bundle: Option<&BundleOptions>) -> Result<Staged, String> {
    match bundle {
        Some(opts) if data.starts_with(GZIP_MAGIC) => stage_bundle(target, data, opts),
        _ => {
            let bytes = appimage_bytes(data, target)?;
            let staged = staged_path(target);
            write_staged(&staged, &mut bytes.as_slice())?;
            let label = bundle::file_label(&target.file_name().unwrap_or_default().to_string_lossy());
            Ok(Staged { files: vec![StagedFile { staged, target: target.to_path_buf(), label }] })
        }
    }
}

fn stage_bundle(target: &Path, data: &[u8], opts: &BundleOptions) -> Result<Staged, String> {
    let members = bundle::members(data)?;
    let plan = bundle::plan(&members, target, opts.app_id.as_deref())?;
    for (name, why) in &plan.skipped {
        eprintln!("linsparkle: leaving {name} alone: {why}");
    }
    let dir = target.parent().unwrap_or(Path::new("."));
    let target_of = |name: &str| -> Option<PathBuf> {
        if name == plan.app {
            Some(target.to_path_buf())
        } else if plan.siblings.iter().any(|s| s == name) {
            Some(dir.join(name))
        } else {
            None
        }
    };
    // The manifest's display name, else the file name it's installed as.
    let label_of = |name: &str, target: &Path| {
        members.iter().find(|m| m.file == name).and_then(|m| m.name.clone()).unwrap_or_else(|| {
            bundle::file_label(&target.file_name().unwrap_or_default().to_string_lossy())
        })
    };
    let mut files: Vec<StagedFile> = Vec::new();
    let result = bundle::walk(data, |name, reader| {
        let Some(target) = target_of(name) else { return Ok(()) };
        let staged = staged_path(&target);
        write_staged(&staged, reader)?;
        let label = label_of(name, &target);
        files.push(StagedFile { staged, target, label });
        Ok(())
    });
    let mut staged = Staged { files };
    if let Err(e) = result {
        discard(staged);
        return Err(e);
    }
    // The running app's file goes first: if it can't be replaced, nothing is.
    let Some(i) = staged.files.iter().position(|f| f.target == target) else {
        discard(staged);
        return Err(format!("the update bundle has nothing for {}", target.display()));
    };
    staged.files.swap(0, i);
    staged.files[1..].sort_by(|a, b| a.target.cmp(&b.target));
    Ok(staged)
}

/// Move the staged files over their targets, the running app's first,
/// keeping each old file's permissions if they were wider (e.g.
/// group-executable). If the running app's can't be replaced, nothing is;
/// if another app's can't be, the rest still are and the error says which.
pub fn install(staged: Staged) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut files = staged.files.into_iter();
    let Some(app) = files.next() else { return Err("there's nothing to install".into()) };
    let replace = |f: &StagedFile| -> Result<(), String> {
        let mode = std::fs::metadata(&f.target).map_or(0o755, |m| m.permissions().mode() | 0o755);
        let _ = std::fs::set_permissions(&f.staged, std::fs::Permissions::from_mode(mode & 0o7777));
        std::fs::rename(&f.staged, &f.target).map_err(|e| {
            let _ = std::fs::remove_file(&f.staged);
            format!("couldn't replace {}: {e}", f.target.display())
        })
    };
    if let Err(e) = replace(&app) {
        for f in files {
            let _ = std::fs::remove_file(&f.staged);
        }
        return Err(e);
    }
    let failed: Vec<String> = files.filter_map(|f| replace(&f).err()).collect();
    let app_name = &app.label;
    if let Ok(dir) = std::fs::File::open(app.target.parent().unwrap_or(Path::new("."))) {
        let _ = dir.sync_all();
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{app_name} was updated (the new version starts the next time it's opened), but the update {}",
            failed.join("; and ")
        ))
    }
}

pub fn discard(staged: Staged) {
    for f in staged.files {
        let _ = std::fs::remove_file(&f.staged);
    }
}

/// Whether the directory holding `target` can take new files, which
/// replacing it (and in bundle mode, the apps beside it) needs.
pub fn dir_writable(target: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let Ok(dir) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: a valid NUL-terminated path, only read.
    unsafe { libc::access(dir.as_ptr(), libc::W_OK) == 0 }
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
    no_inherited_fds(&mut cmd);
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
    no_inherited_fds(&mut cmd);
    if let Err(e) = cmd.spawn() {
        eprintln!("linsparkle: couldn't open {url}: {e}");
    }
}

/// Start `cmd` with only stdin, stdout and stderr. This process inherits
/// the AppImage runtime's keep-alive pipe (and whatever else the app left
/// open), and the runtime keeps its mount until every copy of that pipe is
/// closed: handed on to the relaunched app or a browser, it would keep the
/// replaced AppImage mounted, and its runtime running, as long as they run.
fn no_inherited_fds(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: between fork and exec this only makes async-signal-safe
    // syscalls: no allocation, no locks. Close-on-exec rather than closing,
    // so the Command's own exec-error pipe keeps working until the exec.
    unsafe {
        cmd.pre_exec(|| {
            cloexec_from(3);
            Ok(())
        });
    }
}

fn cloexec_from(first: libc::c_int) {
    // Linux 5.11+ does it in one call.
    #[cfg(target_os = "linux")]
    if unsafe {
        libc::syscall(libc::SYS_close_range, first as libc::c_uint, libc::c_uint::MAX, libc::CLOSE_RANGE_CLOEXEC)
    } == 0
    {
        return;
    }
    let max = match unsafe { libc::sysconf(libc::_SC_OPEN_MAX) } {
        n if n > 0 => n.min(65536) as libc::c_int,
        _ => 1024,
    };
    for fd in first..max {
        // On a descriptor that isn't open this just fails with EBADF.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
        let t = Path::new("/apps/App.AppImage");
        assert_eq!(appimage_bytes(&app, t).unwrap(), app);
        let tgz = tarball(&[("README", b"hi"), ("App-1.2.AppImage", &app)]);
        assert_eq!(appimage_bytes(&tgz, t).unwrap(), app);
        let tgz = tarball(&[("._App.AppImage", b"\x00\x05\x16\x07"), ("App.AppImage", &app)]);
        assert_eq!(appimage_bytes(&tgz, t).unwrap(), app);
        let tgz = tarball(&[("App", &app)]);
        assert_eq!(appimage_bytes(&tgz, t).unwrap(), app);
        assert!(appimage_bytes(&tarball(&[("a", b"x"), ("b", b"y")]), t).is_err());
        assert!(appimage_bytes(&tarball(&[("App.AppImage", b"#!/bin/sh")]), t).is_err());
        assert!(appimage_bytes(b"<html>", t).is_err());
        // Several: the one named like the running app, else the first.
        let other = bundle::tests::elf("other");
        let tgz = tarball(&[("Other.AppImage", &other), ("App.AppImage", &app)]);
        assert_eq!(appimage_bytes(&tgz, t).unwrap(), app);
        assert_eq!(appimage_bytes(&tgz, Path::new("/apps/Renamed.AppImage")).unwrap(), other);
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
    fn children_inherit_no_extra_fds() {
        // Inheritable, like the AppImage runtime's keep-alive pipe. Numbered
        // high so it can't be mistaken for one `ls` opens itself.
        let fd = unsafe { libc::fcntl(2, libc::F_DUPFD, 100) };
        assert!(fd >= 100);
        let child_has_it = |strip: bool| {
            let mut cmd = std::process::Command::new("/bin/sh");
            cmd.arg("-c").arg("ls /dev/fd");
            if strip {
                no_inherited_fds(&mut cmd);
            }
            let out = cmd.output().unwrap();
            String::from_utf8_lossy(&out.stdout).split_whitespace().any(|n| n == fd.to_string())
        };
        assert!(child_has_it(false));
        assert!(!child_has_it(true));
        unsafe { libc::close(fd) };
    }

    #[test]
    fn stages_and_replaces() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("linsparkle-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("App.AppImage");
        std::fs::write(&target, b"old").unwrap();

        let app = fake_appimage();
        let staged = stage(&target, &tarball(&[("App.AppImage", &app)]), None).unwrap();
        let staged_file = staged.files[0].staged.clone();
        assert!(staged_file.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        install(staged).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), app);
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o111, 0o111);
        assert!(!staged_file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Bundle mode.

    use bundle::tests::{bundle as bundle_tgz, elf};

    /// A fresh install directory holding `files` (name, contents), mode 0644.
    fn install_dir(tag: &str, files: &[(&str, &[u8])]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("linsparkle-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, data) in files {
            std::fs::write(dir.join(name), data).unwrap();
            std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        dir
    }

    /// Every file in `dir`, sorted, with its contents.
    fn listing(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let data = std::fs::read(e.path()).unwrap_or_default();
                (e.file_name().to_string_lossy().into_owned(), data)
            })
            .collect();
        v.sort();
        v
    }

    fn suite() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (elf("new loop"), elf("new show"), elf("new trace"))
    }

    fn suite_bundle() -> Vec<u8> {
        let (l, s, t) = suite();
        bundle_tgz(&[
            ("Loop Alchemy.AppImage", &l),
            ("Show Alchemy.AppImage", &s),
            ("Trace Alchemy.AppImage", &t),
            (
                bundle::MANIFEST,
                br#"{"members": [
                    {"file": "Loop Alchemy.AppImage", "id": "loop", "name": "Loop Alchemy Suite"},
                    {"file": "Show Alchemy.AppImage", "id": "show", "name": "Show Alchemy Player"},
                    {"file": "Trace Alchemy.AppImage", "id": "trace"}]}"#,
            ),
        ])
    }

    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o111 == 0o111
    }

    #[test]
    fn bundle_replaces_every_installed_app() {
        let (l, s, t) = suite();
        let dir = install_dir(
            "bundle-all",
            &[("Loop Alchemy.AppImage", b"old loop"), ("Show Alchemy.AppImage", b"old show"), ("Trace Alchemy.AppImage", b"old trace")],
        );
        let target = dir.join("Loop Alchemy.AppImage");
        let opts = BundleOptions::default();
        let staged = stage(&target, &suite_bundle(), Some(&opts)).unwrap();
        // By display name, else file name.
        assert_eq!(staged.others(), ["Show Alchemy Player", "Trace Alchemy"]);
        // Staged, but nothing replaced yet.
        assert_eq!(std::fs::read(&target).unwrap(), b"old loop");
        assert_eq!(listing(&dir).len(), 6);
        install(staged).unwrap();
        assert_eq!(
            listing(&dir),
            [
                ("Loop Alchemy.AppImage".to_string(), l),
                ("Show Alchemy.AppImage".to_string(), s),
                ("Trace Alchemy.AppImage".to_string(), t)
            ]
        );
        for name in ["Loop Alchemy.AppImage", "Show Alchemy.AppImage", "Trace Alchemy.AppImage"] {
            assert!(is_executable(&dir.join(name)), "{name}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundle_skips_apps_that_arent_installed() {
        let (_, s, _) = suite();
        // Run from Show, with Loop missing and Trace a link to elsewhere.
        let dir = install_dir("bundle-missing", &[("Show Alchemy.AppImage", b"old show")]);
        let outside = install_dir("bundle-missing-outside", &[("Trace.AppImage", b"old trace")]);
        std::os::unix::fs::symlink(outside.join("Trace.AppImage"), dir.join("Trace Alchemy.AppImage")).unwrap();
        let target = dir.join("Show Alchemy.AppImage");
        let staged = stage(&target, &suite_bundle(), Some(&BundleOptions::default())).unwrap();
        assert!(staged.others().is_empty());
        install(staged).unwrap();
        let names: Vec<_> = listing(&dir).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["Show Alchemy.AppImage", "Trace Alchemy.AppImage"]);
        assert_eq!(std::fs::read(&target).unwrap(), s);
        assert!(std::fs::symlink_metadata(dir.join("Trace Alchemy.AppImage")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(outside.join("Trace.AppImage")).unwrap(), b"old trace");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn bundle_finds_a_renamed_app_by_id() {
        let (l, s, _) = suite();
        let dir = install_dir("bundle-id", &[("loop_abc123.AppImage", b"old loop"), ("Show Alchemy.AppImage", b"old show")]);
        let target = dir.join("loop_abc123.AppImage");
        assert!(stage(&target, &suite_bundle(), Some(&BundleOptions::default())).is_err());
        assert_eq!(listing(&dir).len(), 2, "a failed plan left files behind");
        let opts = BundleOptions { app_id: Some("loop".into()) };
        install(stage(&target, &suite_bundle(), Some(&opts)).unwrap()).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), l);
        assert_eq!(std::fs::read(dir.join("Show Alchemy.AppImage")).unwrap(), s);
        assert_eq!(listing(&dir).len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundle_staging_failure_replaces_nothing() {
        let old = [
            ("Loop Alchemy.AppImage", &b"old loop"[..]),
            ("Show Alchemy.AppImage", b"old show"),
            ("Trace Alchemy.AppImage", b"old trace"),
        ];
        let dir = install_dir("bundle-fail", &old);
        // Something in the way of Trace's staging file: staging Loop and
        // Show works, then Trace fails.
        let blocker = dir.join(".Trace Alchemy.AppImage.linsparkle-new");
        std::fs::create_dir(&blocker).unwrap();
        let before = listing(&dir);
        let err = stage(&dir.join("Loop Alchemy.AppImage"), &suite_bundle(), Some(&BundleOptions::default())).unwrap_err();
        assert!(err.contains("couldn't write the update"), "{err}");
        assert_eq!(listing(&dir), before, "staging left files behind or replaced some");
        std::fs::remove_dir(&blocker).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundle_reports_an_app_it_couldnt_replace() {
        let (l, s, _) = suite();
        let dir = install_dir(
            "bundle-partial",
            &[("Loop Alchemy.AppImage", b"old loop"), ("Show Alchemy.AppImage", b"old show"), ("Trace Alchemy.AppImage", b"old trace")],
        );
        let target = dir.join("Loop Alchemy.AppImage");
        let staged = stage(&target, &suite_bundle(), Some(&BundleOptions::default())).unwrap();
        // Trace turns into a directory after staging: its rename fails.
        std::fs::remove_file(dir.join("Trace Alchemy.AppImage")).unwrap();
        std::fs::create_dir(dir.join("Trace Alchemy.AppImage")).unwrap();
        let err = install(staged).unwrap_err();
        assert!(err.starts_with("Loop Alchemy Suite was updated"), "{err}");
        assert!(err.contains("Trace Alchemy.AppImage"), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), l);
        assert_eq!(std::fs::read(dir.join("Show Alchemy.AppImage")).unwrap(), s);
        // No staging files left.
        let names: Vec<_> = listing(&dir).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["Loop Alchemy.AppImage", "Show Alchemy.AppImage", "Trace Alchemy.AppImage"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundle_refuses_unsafe_archives_before_writing() {
        let dir = install_dir("bundle-unsafe", &[("Loop Alchemy.AppImage", b"old loop"), ("Show Alchemy.AppImage", b"old show")]);
        let before = listing(&dir);
        let target = dir.join("Loop Alchemy.AppImage");
        let (l, s, _) = suite();
        use tar::EntryType::{Regular, Symlink};
        // The bad entry comes last, after members that would otherwise be
        // staged.
        for tgz in [
            bundle::tests::raw_tarball(&[
                ("Loop Alchemy.AppImage", Regular, &l),
                ("Show Alchemy.AppImage", Regular, &s),
                ("../Escape.AppImage", Regular, &s),
            ]),
            bundle::tests::raw_tarball(&[
                ("Loop Alchemy.AppImage", Regular, &l),
                ("Show Alchemy.AppImage", Symlink, b"/etc/passwd"),
            ]),
        ] {
            assert!(stage(&target, &tgz, Some(&BundleOptions::default())).is_err());
            assert_eq!(listing(&dir), before);
        }
        assert!(!dir.parent().unwrap().join("Escape.AppImage").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundle_mode_takes_a_bare_appimage_or_single_member() {
        let dir = install_dir("bundle-single", &[("Loop Alchemy.AppImage", b"old loop"), ("Show Alchemy.AppImage", b"old show")]);
        let target = dir.join("Loop Alchemy.AppImage");
        let opts = BundleOptions::default();
        let new = elf("bare");
        install(stage(&target, &new, Some(&opts)).unwrap()).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), new);
        let new = elf("single");
        install(stage(&target, &tarball(&[("Loop-2.0-x86_64.AppImage", &new)]), Some(&opts)).unwrap()).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), new);
        assert_eq!(std::fs::read(dir.join("Show Alchemy.AppImage")).unwrap(), b"old show");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_bundle_mode_a_bundle_updates_only_the_running_app() {
        let (_, s, _) = suite();
        let dir = install_dir("bundle-off", &[("Loop Alchemy.AppImage", b"old loop"), ("Show Alchemy.AppImage", b"old show")]);
        let target = dir.join("Show Alchemy.AppImage");
        install(stage(&target, &suite_bundle(), None).unwrap()).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), s);
        assert_eq!(std::fs::read(dir.join("Loop Alchemy.AppImage")).unwrap(), b"old loop");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn notices_a_directory_it_cant_write() {
        use std::os::unix::fs::PermissionsExt;
        let dir = install_dir("readonly", &[("Loop Alchemy.AppImage", b"old loop")]);
        let target = dir.join("Loop Alchemy.AppImage");
        assert!(dir_writable(&target));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root writes anywhere, so there's nothing to see as root.
        if unsafe { libc::geteuid() } != 0 {
            assert!(!dir_writable(&target));
            let err = stage(&target, &suite_bundle(), Some(&BundleOptions::default())).unwrap_err();
            assert!(err.contains("couldn't write the update"), "{err}");
            assert_eq!(std::fs::read(&target).unwrap(), b"old loop");
        } else {
            eprintln!("running as root: skipping the read-only checks");
        }
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bundle_with_a_bad_signature_writes_nothing() {
        use base64::Engine;
        use base64::engine::general_purpose::STANDARD;
        use ed25519_dalek::{Signer, SigningKey};

        let key = SigningKey::from_bytes(&[9u8; 32]);
        let cfg = Config {
            public_key: STANDARD.encode(key.verifying_key().as_bytes()),
            app_name: "Loop Alchemy".into(),
            app_version: "1.0".into(),
            ..Default::default()
        };
        let dir = install_dir("bundle-sig", &[("Loop Alchemy.AppImage", b"old loop"), ("Show Alchemy.AppImage", b"old show")]);
        let before = listing(&dir);
        let target = dir.join("Loop Alchemy.AppImage");
        let tgz = suite_bundle();
        let sig = key.sign(&tgz).to_bytes();
        // Signed by someone else, then a good signature over other bytes.
        let other = SigningKey::from_bytes(&[8u8; 32]).sign(&tgz).to_bytes();
        let mut tampered = tgz.clone();
        let mid = tampered.len() / 2;
        tampered[mid] ^= 1;
        for (body, sig) in [(tgz.clone(), other), (tampered, sig)] {
            let item = Item {
                version: "1.1".into(),
                url: serve_once(body),
                ed_signature: Some(STANDARD.encode(sig)),
                ..Default::default()
            };
            let cancel = AtomicBool::new(false);
            let err = download(&agent(&cfg), &cfg, &item, &cancel, |_, _| {})
                .and_then(|data| stage(&target, &data, Some(&BundleOptions::default())))
                .unwrap_err();
            assert!(err.contains("signature doesn't match"), "{err}");
            assert_eq!(listing(&dir), before);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
