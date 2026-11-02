use std::fs;
use std::io;
use std::path::Path;

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// The parsed content of a .strm file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub value: String,
    /// http(s) URL vs local path
    pub is_remote: bool,
}

/// Extracts the target from a .strm file: the first non-empty,
/// non-comment line. Local paths are returned with `is_remote == false`
/// so the caller can leave them to Plex.
pub fn parse_strm(path: impl AsRef<Path>) -> io::Result<Target> {
    let path = path.as_ref();
    let raw = fs::read(path)?;
    let raw = raw.strip_prefix(BOM).unwrap_or(&raw);
    let text = String::from_utf8_lossy(raw);
    text.split('\n')
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| Target {
            value: line.to_owned(),
            is_remote: is_remote(line),
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: no target line", path.display()),
            )
        })
}

fn is_remote(v: &str) -> bool {
    v.starts_with("http://") || v.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Case {
        name: &'static str,
        content: &'static [u8],
        /// expected (value, is_remote); None = parse error
        want: Option<(&'static str, bool)>,
    }

    #[test]
    fn parse() {
        let dir = tempfile::tempdir().unwrap();
        let cases = [
            Case {
                name: "plain",
                content: b"http://x/f.mkv\n",
                want: Some(("http://x/f.mkv", true)),
            },
            Case {
                name: "bom and comments",
                content: b"\xEF\xBB\xBF# comment\n\n  http://x/f.mkv  \n",
                want: Some(("http://x/f.mkv", true)),
            },
            Case {
                name: "crlf",
                content: b"https://x/f.mkv\r\n",
                want: Some(("https://x/f.mkv", true)),
            },
            Case {
                name: "local path",
                content: b"/media/movies/a.mkv",
                want: Some(("/media/movies/a.mkv", false)),
            },
            Case {
                name: "empty",
                content: b"# only comments\n\n",
                want: None,
            },
        ];
        for Case {
            name,
            content,
            want,
        } in cases
        {
            let path = dir.path().join(format!("{name}.strm"));
            fs::write(&path, content).unwrap();
            let got = parse_strm(&path);
            match want {
                None => assert!(got.is_err(), "{name}: expected error, got {got:?}"),
                Some((value, remote)) => {
                    let got = got.unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert_eq!(got.value, value, "{name}");
                    assert_eq!(got.is_remote, remote, "{name}");
                }
            }
        }
    }

    #[test]
    fn missing_file() {
        let err = parse_strm("/nonexistent/a.strm").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
