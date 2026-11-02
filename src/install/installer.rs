use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::prefs::{PrefsError, set_prefs};

/// The Plex binaries we take over: the scanner is the analysis hook
/// ("--analyze --item N"), the transcoder is the playback hook (its "-i"
/// input is the strm path).
pub const TARGETS: [&str; 2] = ["Plex Media Scanner", "Plex Transcoder"];

const STATE_FILE: &str = "assistant-install.json";

/// A completed install; it lives next to the targets so it disappears
/// together with the container layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub version: String,
    pub binary: PathBuf,
    pub sha256: String,
    pub targets: Vec<String>,
    /// RFC 3339, UTC
    pub installed_at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("target not found: {}", .0.display())]
    TargetNotFound(PathBuf),
    #[error("refusing to remove non-symlink {}", .0.display())]
    NotSymlink(PathBuf),
    #[error("{action} {name}: {source}")]
    Io {
        action: &'static str,
        name: String,
        source: io::Error,
    },
    #[error(transparent)]
    Prefs(#[from] PrefsError),
}

impl InstallError {
    fn io(action: &'static str, name: impl Into<String>) -> impl FnOnce(io::Error) -> Self {
        let name = name.into();
        move |source| Self::Io {
            action,
            name,
            source,
        }
    }
}

/// One target's verification outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub target: &'static str,
    /// None = ok
    pub problem: Option<String>,
}

/// Replaces target binaries inside one Plex directory.
#[derive(Debug, Clone)]
pub struct Installer {
    /// directory containing the target binaries
    pub plex_dir: PathBuf,
    /// Plex data directory (Preferences.xml)
    pub support_dir: PathBuf,
    /// absolute path to this binary
    pub bin_path: PathBuf,
    pub version: String,
}

impl Installer {
    /// Installs all targets. It is idempotent: re-running refreshes the
    /// symlinks without touching the preserved ".real" originals.
    pub fn install(&self) -> Result<(), InstallError> {
        for name in TARGETS {
            let target = self.plex_dir.join(name);
            let real = real_path(&target);

            let target_missing = is_missing(fs::symlink_metadata(&target));
            let real_missing = is_missing(fs::symlink_metadata(&real));
            if target_missing && real_missing {
                return Err(InstallError::TargetNotFound(target));
            }
            if real_missing {
                fs::rename(&target, &real).map_err(InstallError::io("preserve original", name))?;
            }
            // replace any previous symlink (or leftover) with one to us
            remove_if_present(&target).map_err(InstallError::io("clear target", name))?;
            symlink(&self.bin_path, &target).map_err(InstallError::io("link", name))?;
        }

        let state = State {
            version: self.version.clone(),
            binary: self.bin_path.clone(),
            sha256: file_sha256(&self.bin_path).map_err(InstallError::io("hash", "binary"))?,
            targets: TARGETS.map(String::from).to_vec(),
            installed_at: rfc3339_utc(SystemTime::now()),
        };
        let json = serde_json::to_string_pretty(&state).expect("state always serializes");
        fs::write(self.state_path(), json).map_err(InstallError::io("write", "state"))?;
        set_prefs(&self.support_dir)?;
        Ok(())
    }

    /// Checks that every target is our symlink and the original is kept.
    pub fn verify(&self) -> Vec<Check> {
        TARGETS
            .iter()
            .map(|&name| {
                let target = self.plex_dir.join(name);
                let mut problem = match fs::read_link(&target) {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Some("not installed".into()),
                    Err(e) => Some(format!("not a symlink: {e}")),
                    Ok(link) if link != self.bin_path => {
                        Some(format!("points to {}", link.display()))
                    }
                    Ok(_) => None,
                };
                if is_missing(fs::metadata(real_path(&target))) {
                    problem = Some("missing .real original".into());
                }
                Check {
                    target: name,
                    problem,
                }
            })
            .collect()
    }

    /// Restores the original binaries. It only removes targets that are
    /// symlinks, so a misdirected install can never delete a real Plex
    /// file.
    pub fn revert(&self) -> Result<(), InstallError> {
        for name in TARGETS {
            let target = self.plex_dir.join(name);
            let real = real_path(&target);

            if let Ok(meta) = fs::symlink_metadata(&target)
                && !meta.file_type().is_symlink()
            {
                return Err(InstallError::NotSymlink(target));
            }
            if fs::symlink_metadata(&real).is_err() {
                continue; // nothing to revert for this target
            }
            remove_if_present(&target).map_err(InstallError::io("remove symlink", name))?;
            fs::rename(&real, &target).map_err(InstallError::io("restore original", name))?;
        }
        remove_if_present(&self.state_path()).map_err(InstallError::io("remove", "state"))
    }

    /// Returns the recorded install state.
    pub fn state(&self) -> io::Result<State> {
        let raw = fs::read(self.state_path())?;
        serde_json::from_slice(&raw).map_err(io::Error::other)
    }

    fn state_path(&self) -> PathBuf {
        self.plex_dir.join(STATE_FILE)
    }
}

fn real_path(target: &Path) -> PathBuf {
    let mut real = OsString::from(target);
    real.push(".real");
    real.into()
}

fn is_missing<T>(result: io::Result<T>) -> bool {
    matches!(result, Err(e) if e.kind() == io::ErrorKind::NotFound)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn file_sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        match file.read(&mut buf)? {
            0 => break,
            n => hasher.update(&buf[..n]),
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Formats t as RFC 3339 in UTC with second precision.
fn rfc3339_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Fixture {
        _dirs: (tempfile::TempDir, tempfile::TempDir),
        inst: Installer,
        scanner: PathBuf,
    }

    /// A fake plex dir with both target binaries and a fake assistant.
    fn fixture() -> Fixture {
        let plex = tempfile::tempdir().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();
        for name in TARGETS {
            fs::write(plex.path().join(name), "plex original").unwrap();
        }
        let bin = bin_dir.path().join("assistant");
        fs::write(&bin, "assistant bytes").unwrap();
        let inst = Installer {
            plex_dir: plex.path().to_owned(),
            support_dir: plex.path().join("support"),
            bin_path: bin,
            version: "test".into(),
        };
        let scanner = plex.path().join("Plex Media Scanner");
        Fixture {
            _dirs: (plex, bin_dir),
            inst,
            scanner,
        }
    }

    #[test]
    fn install_verify_revert() {
        let Fixture {
            _dirs,
            inst,
            scanner,
        } = fixture();
        inst.install().unwrap();

        // target became a symlink to our binary, original preserved
        assert_eq!(fs::read_link(&scanner).unwrap(), inst.bin_path);
        assert_eq!(
            fs::read_to_string(real_path(&scanner)).unwrap(),
            "plex original"
        );
        assert!(
            inst.verify().iter().all(|c| c.problem.is_none()),
            "{:?}",
            inst.verify()
        );

        let state = inst.state().unwrap();
        assert_eq!(state.version, "test");
        assert_eq!(
            state.sha256,
            "b92c699b3758516e20fcf7e7aab0ce8c0ae767ca799ebf60bdb871602b77b79b"
        );
        assert_eq!(state.targets, TARGETS);
        assert!(state.installed_at.ends_with('Z'));

        inst.revert().unwrap();
        assert_eq!(fs::read_to_string(&scanner).unwrap(), "plex original");
        assert!(!inst.state_path().exists(), "state file removed on revert");
        assert!(inst.verify().iter().all(|c| c.problem.is_some()));
    }

    #[test]
    fn install_is_idempotent() {
        let Fixture {
            _dirs,
            inst,
            scanner,
        } = fixture();
        inst.install().unwrap();
        fs::write(real_path(&scanner), "plex original v2").unwrap();
        inst.install().unwrap();
        assert_eq!(
            fs::read_to_string(real_path(&scanner)).unwrap(),
            "plex original v2",
            "re-install must not clobber the .real original"
        );
    }

    #[test]
    fn missing_target() {
        let Fixture {
            _dirs,
            inst,
            scanner,
        } = fixture();
        fs::remove_file(&scanner).unwrap();
        let err = inst.install().unwrap_err();
        assert!(matches!(err, InstallError::TargetNotFound(_)), "{err:?}");
    }

    #[test]
    fn revert_refuses_non_symlink() {
        let Fixture {
            _dirs,
            inst,
            scanner,
        } = fixture();
        inst.install().unwrap();
        // sneak a real file over our symlink
        fs::remove_file(&scanner).unwrap();
        fs::write(&scanner, "important").unwrap();
        let err = inst.revert().unwrap_err();
        assert!(matches!(err, InstallError::NotSymlink(_)), "{err:?}");
        assert_eq!(fs::read_to_string(&scanner).unwrap(), "important");
    }

    #[test]
    fn verify_reports_problems() {
        let Fixture {
            _dirs,
            inst,
            scanner,
        } = fixture();
        let problems = |inst: &Installer| -> Vec<Option<String>> {
            inst.verify().into_iter().map(|c| c.problem).collect()
        };
        assert_eq!(
            problems(&inst)[0].as_deref(),
            Some("missing .real original")
        );

        inst.install().unwrap();
        fs::remove_file(&scanner).unwrap();
        symlink("/elsewhere", &scanner).unwrap();
        assert_eq!(problems(&inst)[0].as_deref(), Some("points to /elsewhere"));

        fs::remove_file(&scanner).unwrap();
        assert_eq!(problems(&inst)[0].as_deref(), Some("not installed"));
        assert_eq!(problems(&inst)[1], None, "transcoder still fine");
    }

    #[test]
    fn install_sets_prefs() {
        let Fixture { _dirs, inst, .. } = fixture();
        fs::create_dir_all(&inst.support_dir).unwrap();
        let prefs = inst.support_dir.join("Preferences.xml");
        fs::write(&prefs, "<Preferences/>").unwrap();
        inst.install().unwrap();
        assert!(
            fs::read_to_string(prefs)
                .unwrap()
                .contains(r#"FSEventLibraryUpdatesEnabled="1""#)
        );
    }

    #[test]
    fn timestamps() {
        // expected values from `date -u -d @N`
        let at = |secs| rfc3339_utc(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_793_612_219), "2026-11-02T09:36:59Z");
    }
}
