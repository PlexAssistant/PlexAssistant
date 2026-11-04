//! Replaces Plex's scanner and transcoder binaries with symlinks to this
//! binary, records the installation for verify and revert, and sets the
//! library-scan preferences the integration relies on.

mod installer;
mod prefs;

pub use installer::{Check, InstallError, Installer, ORIGINAL_SUFFIX, State, TARGETS};
pub use prefs::{DEFAULT_SUPPORT_DIR, PrefsError, set_prefs};
