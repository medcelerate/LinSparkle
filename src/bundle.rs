//! Bundle mode: one update for a suite of AppImages installed side by side.
//!
//! A bundle is a `.tar.gz` (signed as a whole, like any update) holding
//! several AppImages and, optionally, a `linsparkle-bundle.json` manifest:
//!
//! ```json
//! { "members": [
//!     { "file": "Loop Alchemy.AppImage", "id": "loop-alchemy", "name": "Loop Alchemy" },
//!     { "file": "Show Alchemy.AppImage", "id": "show-alchemy", "name": "Show Alchemy" }
//! ] }
//! ```
//!
//! `id` and `name` are optional. `name` is what the update window calls the
//! member; without one, it uses the file name less `.AppImage`
//! (`file_label`).
//!
//! Without a manifest, every ELF file in the archive is a member. The
//! running AppImage is replaced by its member (the one with the app id set
//! by `set_bundle_app_id`, else the one with the same file name, else the
//! only one); every other member replaces the file of the same name in the
//! same directory, and only if that file is already there.
//!
//! The archive must be flat: every entry a regular file with a plain name
//! (a leading `./` is allowed, as is the `./` entry itself). Links, other
//! directories, devices, `..` and names with a `/` make the whole update
//! fail before anything is written.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use tar::EntryType;

pub const MANIFEST: &str = "linsparkle-bundle.json";
const MAX_MANIFEST: u64 = 64 * 1024;
const ELF_MAGIC: &[u8] = b"\x7fELF";
/// The longest display name a manifest may give, in characters.
pub const MAX_DISPLAY_NAME: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub file: String,
    pub id: Option<String>,
    /// The display name from the manifest, if it gives one.
    pub name: Option<String>,
}

/// A file name as an app name: without its `.AppImage` (in any case).
pub fn file_label(file: &str) -> String {
    let cut = file.len().saturating_sub(".AppImage".len());
    match file.get(cut..) {
        Some(ext) if cut > 0 && ext.eq_ignore_ascii_case(".AppImage") => file[..cut].to_string(),
        _ => file.to_string(),
    }
}

/// A display name: plain text on one line, at most `MAX_DISPLAY_NAME`
/// characters once trimmed, none of them control or bidirectional
/// formatting characters (which could reorder the text around it).
pub fn check_display_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let bidi = |c: char| matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let line_break = |c: char| matches!(c, '\u{2028}' | '\u{2029}');
    if name.is_empty() {
        return Err("is empty".into());
    }
    if name.chars().count() > MAX_DISPLAY_NAME {
        return Err(format!("is longer than {MAX_DISPLAY_NAME} characters"));
    }
    if name.chars().any(|c| c.is_control() || bidi(c) || line_break(c)) {
        return Err("has a control character".into());
    }
    Ok(name.to_string())
}

/// What a bundle will do in one install directory.
#[derive(Debug, PartialEq)]
pub struct Plan {
    /// The member that replaces the running AppImage.
    pub app: String,
    /// Members that replace a file of the same name next to it.
    pub siblings: Vec<String>,
    /// Members left alone, and why (not installed here, or not a plain file).
    pub skipped: Vec<(String, &'static str)>,
}

/// A plain file name: no separators, no `..`, not hidden (which also keeps
/// out macOS's `._` AppleDouble files and LinSparkle's own staging files).
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || name.contains('\0') || name.starts_with('.') || name.len() > 255 {
        return Err(format!("the update bundle names a file “{name}”, which isn't a plain file name"));
    }
    Ok(())
}

/// An entry's name with any leading `./`; `None` for the archive's root.
fn entry_name(raw: &[u8]) -> Result<Option<String>, String> {
    let name = std::str::from_utf8(raw)
        .map_err(|_| format!("the update bundle has a file name that isn't UTF-8: {}", String::from_utf8_lossy(raw)))?;
    let name = name.strip_prefix("./").unwrap_or(name);
    Ok(if name.is_empty() || name == "." { None } else { Some(name.to_string()) })
}

/// Walk every entry of a bundle, checking each is allowed; `visit` is
/// called with each regular file's name and its contents reader.
pub fn walk(
    data: &[u8],
    mut visit: impl FnMut(&str, &mut dyn Read) -> Result<(), String>,
) -> Result<(), String> {
    let read_err = |e: std::io::Error| format!("couldn't read the update bundle: {e}");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
    let mut seen = HashSet::new();
    for entry in archive.entries().map_err(read_err)? {
        let mut entry = entry.map_err(read_err)?;
        let kind = entry.header().entry_type();
        let name = entry_name(&entry.path_bytes())?;
        match kind {
            // Extended headers that tar folds into the next entry.
            EntryType::XGlobalHeader | EntryType::XHeader | EntryType::GNULongName | EntryType::GNULongLink => {
                continue;
            }
            EntryType::Directory if name.is_none() => continue,
            EntryType::Regular | EntryType::Continuous => {}
            EntryType::Symlink | EntryType::Link => {
                return Err(format!(
                    "the update bundle contains a link ({}); only plain files are allowed",
                    name.unwrap_or_default()
                ));
            }
            _ => {
                return Err(format!(
                    "the update bundle contains something other than a plain file ({}); only plain files are allowed",
                    name.unwrap_or_default()
                ));
            }
        }
        let Some(name) = name else {
            return Err("the update bundle contains a file with no name".into());
        };
        if name.contains('/') || name == ".." {
            return Err(format!("the update bundle contains “{name}”; only files at its top level are allowed"));
        }
        if !seen.insert(name.clone()) {
            return Err(format!("the update bundle contains “{name}” twice"));
        }
        visit(&name, &mut entry)?;
    }
    Ok(())
}

fn parse_manifest(text: &[u8]) -> Result<Vec<Member>, String> {
    let bad = |why: &str| format!("the update bundle's {MANIFEST} {why}");
    let json: serde_json::Value = serde_json::from_slice(text).map_err(|e| bad(&format!("isn't valid JSON: {e}")))?;
    let list = json
        .get("members")
        .and_then(|m| m.as_array())
        .ok_or_else(|| bad("has no \"members\" list"))?;
    let mut members = Vec::new();
    for m in list {
        let file = m
            .get("file")
            .and_then(|f| f.as_str())
            .ok_or_else(|| bad("has a member without a \"file\""))?;
        let id = match m.get("id") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(id)) => Some(id.clone()),
            Some(_) => return Err(bad("has a member whose \"id\" isn't a string")),
        };
        let name = match m.get("name") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(name)) => Some(
                check_display_name(name).map_err(|why| bad(&format!("has a \"name\" for “{file}” that {why}")))?,
            ),
            Some(_) => return Err(bad("has a member whose \"name\" isn't a string")),
        };
        check_name(file)?;
        if members.iter().any(|o: &Member| o.file == file) {
            return Err(bad(&format!("lists “{file}” twice")));
        }
        if id.is_some() && members.iter().any(|o: &Member| o.id == id) {
            return Err(bad(&format!("uses the id “{}” twice", id.unwrap_or_default())));
        }
        members.push(Member { file: file.to_string(), id, name });
    }
    if members.is_empty() {
        return Err(bad("lists no members"));
    }
    Ok(members)
}

/// The members of a (verified) bundle, after checking every entry: those
/// in its manifest, else every ELF file at its top level.
pub fn members(data: &[u8]) -> Result<Vec<Member>, String> {
    let mut manifest = None;
    let mut elves = Vec::new();
    walk(data, |name, reader| {
        if name == MANIFEST {
            let mut text = Vec::new();
            reader
                .take(MAX_MANIFEST + 1)
                .read_to_end(&mut text)
                .map_err(|e| format!("couldn't read the update bundle: {e}"))?;
            if text.len() as u64 > MAX_MANIFEST {
                return Err(format!("the update bundle's {MANIFEST} is too large"));
            }
            manifest = Some(text);
            return Ok(());
        }
        let mut magic = [0u8; 4];
        let mut got = 0;
        while got < magic.len() {
            match reader.read(&mut magic[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) => return Err(format!("couldn't read the update bundle: {e}")),
            }
        }
        if magic[..got] == *ELF_MAGIC {
            elves.push(name.to_string());
        }
        Ok(())
    })?;
    let members = match manifest {
        Some(text) => {
            let members = parse_manifest(&text)?;
            if let Some(m) = members.iter().find(|m| !elves.contains(&m.file)) {
                return Err(format!(
                    "the update bundle lists “{}” but doesn't contain it as an executable",
                    m.file
                ));
            }
            members
        }
        None => elves
            .into_iter()
            .filter(|name| !name.starts_with('.'))
            .map(|file| Member { file, id: None, name: None })
            .collect(),
    };
    for m in &members {
        check_name(&m.file)?;
    }
    if members.is_empty() {
        return Err("the update bundle doesn't contain an AppImage".into());
    }
    Ok(members)
}

/// Which members to install, given the running AppImage `target`: its own
/// member, then each other member whose file is already in its directory.
pub fn plan(members: &[Member], target: &Path, app_id: Option<&str>) -> Result<Plan, String> {
    let target_name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = target.parent().unwrap_or(Path::new("."));
    let app = app_id
        .filter(|id| !id.is_empty())
        .and_then(|id| members.iter().find(|m| m.id.as_deref() == Some(id)))
        .or_else(|| members.iter().find(|m| m.file == target_name))
        .or_else(|| (members.len() == 1).then(|| &members[0]))
        .ok_or_else(|| format!("the update bundle has nothing for “{target_name}”"))?;
    let mut siblings = Vec::new();
    let mut skipped = Vec::new();
    for m in members.iter().filter(|m| m.file != app.file) {
        if m.file == target_name {
            // Another member named like the running app: never put it there.
            skipped.push((m.file.clone(), "named like the running app"));
            continue;
        }
        match std::fs::symlink_metadata(dir.join(&m.file)) {
            Ok(meta) if meta.file_type().is_file() => siblings.push(m.file.clone()),
            Ok(_) => skipped.push((m.file.clone(), "not a plain file here")),
            Err(_) => skipped.push((m.file.clone(), "not installed here")),
        }
    }
    Ok(Plan { app: app.file.clone(), siblings, skipped })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn elf(tag: &str) -> Vec<u8> {
        let mut v = ELF_MAGIC.to_vec();
        v.extend_from_slice(tag.as_bytes());
        v
    }

    /// A tarball of entries: (name, type, contents or link target).
    pub fn raw_tarball(entries: &[(&str, EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (name, kind, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_mode(0o755);
            match kind {
                EntryType::Symlink | EntryType::Link => {
                    header.set_size(0);
                    // Set the name in the header directly so the builder
                    // doesn't refuse (or tidy up) what we're testing.
                    header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
                    header.set_link_name(std::str::from_utf8(data).unwrap()).unwrap();
                    header.set_cksum();
                    builder.append(&header, std::io::empty()).unwrap();
                }
                _ => {
                    header.set_size(data.len() as u64);
                    header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
                    header.set_cksum();
                    builder.append(&header, *data).unwrap();
                }
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    pub fn bundle(files: &[(&str, &[u8])]) -> Vec<u8> {
        let entries: Vec<_> = files.iter().map(|(n, d)| (*n, EntryType::Regular, *d)).collect();
        raw_tarball(&entries)
    }

    fn names(members: &[Member]) -> Vec<&str> {
        members.iter().map(|m| m.file.as_str()).collect()
    }

    #[test]
    fn members_without_a_manifest_are_its_executables() {
        let (a, b) = (elf("loop"), elf("show"));
        let tgz = bundle(&[
            ("Loop.AppImage", &a),
            ("._Loop.AppImage", b"\x00\x05\x16\x07"),
            ("README", b"hi"),
            ("./Show.AppImage", &b),
        ]);
        assert_eq!(names(&members(&tgz).unwrap()), ["Loop.AppImage", "Show.AppImage"]);
        // `tar czf b.tgz -C dir .` adds the root directory too.
        let tgz = raw_tarball(&[("./", EntryType::Directory, b""), ("./Loop.AppImage", EntryType::Regular, &a)]);
        assert_eq!(names(&members(&tgz).unwrap()), ["Loop.AppImage"]);
        assert!(members(&bundle(&[("README", b"hi")])).is_err());
    }

    #[test]
    fn members_from_the_manifest() {
        let (a, b, c) = (elf("loop"), elf("show"), elf("extra"));
        let manifest = br#"{"members": [{"file": "Show.AppImage", "id": "show"}, {"file": "Loop.AppImage"}]}"#;
        let tgz = bundle(&[
            ("Loop.AppImage", &a),
            ("Show.AppImage", &b),
            ("helper", &c),
            (MANIFEST, manifest),
        ]);
        let m = members(&tgz).unwrap();
        assert_eq!(names(&m), ["Show.AppImage", "Loop.AppImage"]);
        assert_eq!(m[0].id.as_deref(), Some("show"));
        assert_eq!(m[1].id, None);
        assert_eq!((m[0].name.as_deref(), m[1].name.as_deref()), (None, None));

        // Listed but missing, or not an executable.
        let tgz = bundle(&[("Loop.AppImage", &a), (MANIFEST, manifest)]);
        assert!(members(&tgz).unwrap_err().contains("Show.AppImage"));
        let tgz = bundle(&[("Loop.AppImage", &a), ("Show.AppImage", b"#!/bin/sh"), (MANIFEST, manifest)]);
        assert!(members(&tgz).is_err());

        for bad in [
            &br#"{"members": [{"file": "../Evil.AppImage"}]}"#[..],
            br#"{"members": [{"file": "sub/Evil.AppImage"}]}"#,
            br#"{"members": [{"file": ".."}]}"#,
            br#"{"members": [{"file": ".hidden"}]}"#,
            br#"{"members": [{"file": ""}]}"#,
            br#"{"members": [{"file": "Loop.AppImage"}, {"file": "Loop.AppImage"}]}"#,
            br#"{"members": []}"#,
            br#"{"members": [{"id": "x"}]}"#,
            br#"{"members": [{"file": "Loop.AppImage", "id": 3}]}"#,
            br#"{"members": [{"file": "Loop.AppImage", "name": 3}]}"#,
            br#"{"members": [{"file": "Loop.AppImage", "name": ["Loop"]}]}"#,
            br#"{"members": [{"file": "Loop.AppImage", "name": "  "}]}"#,
            br#"{"members": [{"file": "Loop.AppImage", "name": "Loop\nAlchemy"}]}"#,
            br#"["Loop.AppImage"]"#,
            b"not json",
        ] {
            let tgz = bundle(&[("Loop.AppImage", &a), (MANIFEST, bad)]);
            assert!(members(&tgz).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn members_with_display_names() {
        let (a, b, c) = (elf("loop"), elf("show"), elf("trace"));
        let manifest = br#"{"members": [
            {"file": "Excalibur_Loop_Alchemy-x86_64.AppImage", "id": "loop", "name": "Loop Alchemy"},
            {"file": "Excalibur_Show_Alchemy-x86_64.AppImage", "name": "Show Alchemy"},
            {"file": "Excalibur_Trace_Alchemy-x86_64.AppImage", "id": "trace"}]}"#;
        let tgz = bundle(&[
            ("Excalibur_Loop_Alchemy-x86_64.AppImage", &a),
            ("Excalibur_Show_Alchemy-x86_64.AppImage", &b),
            ("Excalibur_Trace_Alchemy-x86_64.AppImage", &c),
            (MANIFEST, manifest),
        ]);
        let m = members(&tgz).unwrap();
        assert_eq!(m[0].name.as_deref(), Some("Loop Alchemy"));
        assert_eq!(m[1].id, None);
        assert_eq!(m[2].name, None);
        assert_eq!(m[1].name.as_deref(), Some("Show Alchemy"));
    }

    #[test]
    fn display_names() {
        assert_eq!(check_display_name("Show Alchemy").unwrap(), "Show Alchemy");
        assert_eq!(check_display_name("  Show Alchemy\t").unwrap(), "Show Alchemy");
        assert_eq!(check_display_name("Éditeur — Ünïcode 編集").unwrap(), "Éditeur — Ünïcode 編集");
        let longest = "é".repeat(MAX_DISPLAY_NAME);
        assert_eq!(check_display_name(&longest).unwrap(), longest);
        let too_long = "x".repeat(MAX_DISPLAY_NAME + 1);
        for bad in [
            "",
            " \t ",
            &too_long,
            "Show\nAlchemy",
            "Show\rAlchemy",
            "Show\0Alchemy",
            "Show\u{1b}[31mAlchemy",
            "Show\u{7f}",
            "Show\u{85}Alchemy",
            "Show\u{2028}Alchemy",
            "Show \u{202e}ymehclA",
            "Show \u{2067}Alchemy\u{2069}",
            "Show\u{200f}",
        ] {
            assert!(check_display_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn file_labels() {
        assert_eq!(file_label("Show Alchemy.AppImage"), "Show Alchemy");
        assert_eq!(file_label("Trace Alchemy.appimage"), "Trace Alchemy");
        assert_eq!(file_label("helper"), "helper");
        assert_eq!(file_label(".AppImage"), ".AppImage");
        assert_eq!(file_label("Ünï.AppImage"), "Ünï");
        assert_eq!(file_label("ü"), "ü");
    }

    #[test]
    fn refuses_unsafe_archives() {
        let a = elf("loop");
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("parent", raw_tarball(&[("../Loop.AppImage", EntryType::Regular, &a)])),
            ("nested parent", raw_tarball(&[("x/../../Loop.AppImage", EntryType::Regular, &a)])),
            ("subdir", raw_tarball(&[("sub/Loop.AppImage", EntryType::Regular, &a)])),
            ("absolute", raw_tarball(&[("/tmp/Loop.AppImage", EntryType::Regular, &a)])),
            ("dotdot", raw_tarball(&[("..", EntryType::Regular, &a)])),
            ("directory", raw_tarball(&[("sub/", EntryType::Directory, b""), ("Loop.AppImage", EntryType::Regular, &a)])),
            (
                "symlink",
                raw_tarball(&[("Loop.AppImage", EntryType::Regular, &a), ("Show.AppImage", EntryType::Symlink, b"/etc/passwd")]),
            ),
            (
                "hard link",
                raw_tarball(&[("Loop.AppImage", EntryType::Regular, &a), ("Show.AppImage", EntryType::Link, b"Loop.AppImage")]),
            ),
            ("fifo", raw_tarball(&[("Loop.AppImage", EntryType::Regular, &a), ("pipe", EntryType::Fifo, b"")])),
            (
                "duplicate",
                raw_tarball(&[("Loop.AppImage", EntryType::Regular, &a), ("./Loop.AppImage", EntryType::Regular, &a)]),
            ),
        ];
        for (what, tgz) in cases {
            assert!(members(&tgz).is_err(), "{what} was accepted");
        }
        assert!(members(b"\x1f\x8bnot really gzip").is_err());
    }

    #[test]
    fn plain_names() {
        for ok in ["Loop Alchemy.AppImage", "a", "Show-1.2_x86_64.AppImage"] {
            check_name(ok).unwrap();
        }
        for bad in ["", ".", "..", "a/b", "/a", ".hidden", "._A.AppImage", "a\0b"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn plans_by_name_and_id() {
        let dir = std::env::temp_dir().join(format!("linsparkle-plan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Loop.AppImage"), b"old").unwrap();
        std::fs::write(dir.join("Show.AppImage"), b"old").unwrap();
        std::fs::create_dir_all(dir.join("Odd.AppImage")).unwrap();
        let m = |file: &str, id: Option<&str>| Member { file: file.into(), id: id.map(Into::into), name: None };
        let members =
            [m("Loop.AppImage", Some("loop")), m("Show.AppImage", None), m("Trace.AppImage", None), m("Odd.AppImage", None)];

        let p = plan(&members, &dir.join("Loop.AppImage"), None).unwrap();
        assert_eq!(p.app, "Loop.AppImage");
        assert_eq!(p.siblings, ["Show.AppImage"]);
        assert_eq!(p.skipped.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["Trace.AppImage", "Odd.AppImage"]);

        // Renamed (say by AppImageLauncher): found by id instead.
        std::fs::write(dir.join("loop_1234.AppImage"), b"old").unwrap();
        let p = plan(&members, &dir.join("loop_1234.AppImage"), Some("loop")).unwrap();
        assert_eq!(p.app, "Loop.AppImage");
        assert_eq!(p.siblings, ["Show.AppImage"]);
        assert!(plan(&members, &dir.join("loop_1234.AppImage"), None).is_err());
        assert!(plan(&members, &dir.join("loop_1234.AppImage"), Some("nope")).is_err());

        // A single member is the app, whatever its name (as before bundles).
        let p = plan(&members[1..2], &dir.join("loop_1234.AppImage"), None).unwrap();
        assert_eq!(p.app, "Show.AppImage");
        assert!(p.siblings.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
