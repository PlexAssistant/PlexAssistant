//! The credits-detection model Plex already keeps on disk.
//!
//! Plex downloads `model_v1.pb` into its scanners directory the first time
//! it runs credits detection. This module only checks that copy. It does
//! not fetch another one, and it does not run detection: a missing or
//! damaged file is the caller's signal to stay on the heuristic.

use std::io;
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

const MODEL_FILE: &str = "model_v1.pb";

/// Where Plex stores the model, under the config root.
const RELATIVE: &str = "Library/Application Support/Plex Media Server/Scanners/Credits Detection";

/// SHA-1 of the published `model_v1.pb`. Anything else is not usable.
pub const MODEL_SHA1: &str = "fffabf5ac36f37bddf68e34e84b45f5c4247ed06";

/// Plex's config root inside the container.
pub const DEFAULT_CONFIG: &str = "/config";

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("credits model: read {path}: {source}")]
    Io { path: String, source: io::Error },
}

/// A copy of the model that matches [`MODEL_SHA1`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub path: PathBuf,
    pub sha1: String,
}

/// The model file under Plex's config root.
pub fn path(config: &Path) -> PathBuf {
    config.join(RELATIVE).join(MODEL_FILE)
}

/// The model at `path` when it exists and matches [`MODEL_SHA1`].
///
/// Missing and a wrong checksum are both `Ok(None)`: detection falls back
/// either way. Only a read error is fatal.
pub async fn load(path: &Path) -> Result<Option<Model>, ModelError> {
    checked(path, MODEL_SHA1).await
}

async fn checked(path: &Path, expected: &str) -> Result<Option<Model>, ModelError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(ModelError::Io {
                path: path.display().to_string(),
                source: err,
            });
        }
    };
    let sha1 = hex(&Sha1::digest(&bytes));
    Ok((sha1 == expected).then(|| Model {
        path: path.to_owned(),
        sha1,
    }))
}

fn hex(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_is_under_plex_scanners() {
        assert_eq!(
            path(Path::new("/config")),
            Path::new(
                "/config/Library/Application Support/Plex Media Server/Scanners/Credits Detection/model_v1.pb"
            )
        );
    }

    #[tokio::test]
    async fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(MODEL_FILE);
        assert_eq!(load(&file).await.unwrap(), None);
    }

    #[tokio::test]
    async fn wrong_checksum_is_not_usable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(MODEL_FILE);
        tokio::fs::write(&file, b"not the model").await.unwrap();
        assert_eq!(load(&file).await.unwrap(), None);
    }

    #[tokio::test]
    async fn matching_checksum_is_returned() {
        // the published file is ~92MB; the accept path is the same compare,
        // run here against a stand-in digest
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(MODEL_FILE);
        let bytes = b"credits-model-stand-in";
        tokio::fs::write(&file, bytes).await.unwrap();
        let sha1 = hex(&Sha1::digest(bytes));
        let found = checked(&file, &sha1).await.unwrap().expect("match");
        assert_eq!(found.path, file);
        assert_eq!(found.sha1, sha1);
    }

    #[test]
    fn published_checksum_is_lowercase_hex() {
        assert_eq!(MODEL_SHA1.len(), 40);
        assert!(MODEL_SHA1.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(MODEL_SHA1, MODEL_SHA1.to_ascii_lowercase());
    }
}
