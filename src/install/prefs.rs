use std::ffi::OsString;
use std::fmt::Write;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::Event;

/// The Plex data directory inside the container.
pub const DEFAULT_SUPPORT_DIR: &str = "/config/Library/Application Support/Plex Media Server";

/// Set at install time so library scans react to file changes
/// immediately.
const INSTALL_PREFS: [(&str, &str); 2] = [
    ("FSEventLibraryUpdatesEnabled", "1"),
    ("FSEventLibraryPartialScanEnabled", "1"),
];

#[derive(Debug, thiserror::Error)]
pub enum PrefsError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("parse {}: {reason}", path.display())]
    Parse { path: PathBuf, reason: String },
}

/// Ensures our preference attributes are present on the Preferences.xml
/// root element. Only that start tag is rewritten: the XML declaration,
/// every other attribute and the rest of the file stay byte for byte, and
/// the file keeps its owner and mode (install runs as root, Plex does not).
/// A missing file is not an error (Plex has not booted yet).
pub fn set_prefs(support_dir: impl AsRef<Path>) -> Result<(), PrefsError> {
    let path = support_dir.as_ref().join("Preferences.xml");
    let xml = match fs::read_to_string(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        other => other?,
    };
    let updated = apply_prefs(&xml).map_err(|reason| PrefsError::Parse {
        path: path.clone(),
        reason,
    })?;
    match updated {
        Some(updated) => replace_file(&path, &updated).map_err(Into::into),
        None => Ok(()),
    }
}

/// Returns the document with our attributes set on the root element, or
/// None when they already are.
fn apply_prefs(xml: &str) -> Result<Option<String>, String> {
    let mut reader = Reader::from_str(xml);
    loop {
        let start = reader.buffer_position() as usize;
        let (tag, empty) = match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(tag) => (tag, false),
            Event::Empty(tag) => (tag, true),
            Event::Eof => return Err("no root element".into()),
            _ => continue,
        };
        let end = reader.buffer_position() as usize;

        let mut attrs = tag
            .attributes()
            .map(|a| {
                let a = a.map_err(|e| e.to_string())?;
                Ok((a.key.into_inner().to_owned(), a.value.into_owned()))
            })
            .collect::<Result<Vec<(String, String)>, String>>()?;
        let mut changed = false;
        for (key, want) in INSTALL_PREFS {
            match attrs.iter_mut().find(|(k, _)| k == key) {
                Some((_, value)) if value == want => {}
                Some((_, value)) => {
                    *value = want.to_owned();
                    changed = true;
                }
                None => {
                    attrs.push((key.to_owned(), want.to_owned()));
                    changed = true;
                }
            }
        }
        if !changed {
            return Ok(None);
        }

        // values stay raw (already escaped); a raw value holding '"' came
        // from a single-quoted attribute and keeps that quoting
        let mut out = String::with_capacity(xml.len() + 96);
        out.push_str(&xml[..start]);
        out.push('<');
        out.push_str(tag.name().into_inner());
        for (key, value) in &attrs {
            let quote = if value.contains('"') { '\'' } else { '"' };
            let _ = write!(out, " {key}={quote}{value}{quote}");
        }
        out.push_str(if empty { "/>" } else { ">" });
        out.push_str(&xml[end..]);
        return Ok(Some(out));
    }
}

/// Atomically replaces path, carrying over its owner and permissions.
fn replace_file(path: &Path, contents: &str) -> io::Result<()> {
    let meta = fs::metadata(path)?;
    let mut tmp = OsString::from(path);
    tmp.push(".assistant-tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        fs::write(&tmp, contents)?;
        fs::set_permissions(&tmp, meta.permissions())?;
        std::os::unix::fs::chown(&tmp, Some(meta.uid()), Some(meta.gid()))?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn prefs_file(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Preferences.xml");
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn sets_and_preserves() {
        let (dir, path) = prefs_file(r#"<Preferences FriendlyName="rig" LogLevel="debug"/>"#);
        set_prefs(dir.path()).unwrap();
        let s = fs::read_to_string(&path).unwrap();
        assert_eq!(
            s,
            r#"<Preferences FriendlyName="rig" LogLevel="debug" FSEventLibraryUpdatesEnabled="1" FSEventLibraryPartialScanEnabled="1"/>"#
        );

        // idempotent second run changes nothing
        set_prefs(dir.path()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), s);
    }

    #[test]
    fn overwrites_wrong_value() {
        let (dir, path) = prefs_file(r#"<Preferences FSEventLibraryUpdatesEnabled="0"/>"#);
        set_prefs(dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"<Preferences FSEventLibraryUpdatesEnabled="1" FSEventLibraryPartialScanEnabled="1"/>"#
        );
    }

    #[test]
    fn missing_file_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        set_prefs(dir.path()).unwrap();
        assert!(!dir.path().join("Preferences.xml").exists());
    }

    #[test]
    fn keeps_the_rest_of_the_document() {
        let (dir, path) = prefs_file(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- kept -->\n\
             <Preferences Name=\"a &amp; b\" Quote='say \"hi\"'>\n  <Child x=\"1\"/>\n</Preferences>\n",
        );
        set_prefs(dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- kept -->\n\
             <Preferences Name=\"a &amp; b\" Quote='say \"hi\"' \
             FSEventLibraryUpdatesEnabled=\"1\" FSEventLibraryPartialScanEnabled=\"1\">\n  \
             <Child x=\"1\"/>\n</Preferences>\n"
        );
    }

    #[test]
    fn keeps_permissions() {
        let (dir, path) = prefs_file("<Preferences/>");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        set_prefs(dir.path()).unwrap();
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert!(!dir.path().join("Preferences.xml.assistant-tmp").exists());
    }

    #[test]
    fn malformed_file_is_an_error() {
        for bad in ["", "not xml at all", "<Preferences a=\"1\" a=\"2\"/>"] {
            let (dir, path) = prefs_file(bad);
            let err = set_prefs(dir.path()).unwrap_err();
            assert!(matches!(err, PrefsError::Parse { .. }), "{bad:?}: {err:?}");
            assert_eq!(fs::read_to_string(&path).unwrap(), bad, "left untouched");
        }
    }
}
