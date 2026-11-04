//! Plex's scanner argv: the few flags the assistant acts on.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::cli::env_or;
use crate::linkres::DEFAULT_UA;
use crate::plexdb;
use crate::runtime::DEFAULT_SOCK;

/// The flags we act on; Plex passes many more, which are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScanFlags {
    pub(super) analyze: bool,
    pub(super) section: i64,
    pub(super) item: i64,
    pub(super) library_db: PathBuf,
    pub(super) ffprobe: String,
    pub(super) ua: String,
    pub(super) runtime_sock: PathBuf,
}

impl Default for ScanFlags {
    fn default() -> Self {
        Self {
            analyze: false,
            section: 0,
            item: 0,
            library_db: plexdb::DEFAULT_PATH.into(),
            ffprobe: env_or("ASSISTANT_FFPROBE", super::super::DEFAULT_FFPROBE),
            ua: DEFAULT_UA.into(),
            runtime_sock: DEFAULT_SOCK.into(),
        }
    }
}

/// Parses Plex's scanner argv with pflag's rules for whitelisted unknown
/// flags: `--name=value` or `--name value` (a known flag takes the next
/// argument whatever it is), and an unknown flag without `=` swallows the
/// next argument unless that looks like a flag, so a later `--item N` is
/// never mistaken for its value.
pub(super) fn parse_flags(args: &[OsString]) -> Result<ScanFlags, String> {
    let mut flags = ScanFlags::default();
    let mut rest = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .peekable();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break; // positional arguments only from here on
        }
        // a lone "-" is positional, like everything not starting with '-'
        let Some(flag) = arg
            .strip_prefix("--")
            .or_else(|| arg.strip_prefix('-'))
            .filter(|f| !f.is_empty())
        else {
            continue; // positional: ignored
        };
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (flag, None),
        };
        let looks_like_value = |next: &Option<&String>| next.is_some_and(|n| !n.starts_with('-'));
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| rest.next()) // a known flag takes the next arg as-is
                .ok_or_else(|| format!("flag needs an argument: --{name}"))
        };
        let int = |name: &str, v: String| {
            v.parse::<i64>()
                .map_err(|_| format!("invalid argument {v:?} for --{name}"))
        };
        match name {
            "analyze" => {
                flags.analyze = match inline.as_deref() {
                    None | Some("true" | "1" | "t" | "T" | "TRUE" | "True") => true,
                    Some("false" | "0" | "f" | "F" | "FALSE" | "False") => false,
                    Some(v) => return Err(format!("invalid argument {v:?} for --analyze")),
                }
            }
            "section" => flags.section = int(name, value(name)?)?,
            "item" => flags.item = int(name, value(name)?)?,
            "library-db" => flags.library_db = value(name)?.into(),
            "ffprobe" => flags.ffprobe = value(name)?,
            "ua" => flags.ua = value(name)?,
            "runtime-sock" => flags.runtime_sock = value(name)?.into(),
            _ => {
                if inline.is_none() && looks_like_value(&rest.peek()) {
                    rest.next();
                }
            }
        }
    }
    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<ScanFlags, String> {
        parse_flags(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn plex_style_argv() {
        let f = parse(&[
            "--analyze",
            "--log-file-suffix",
            " Analysis",
            "--item",
            "123",
            "--quick",
            "-x",
            "positional",
        ])
        .unwrap();
        assert!(f.analyze);
        assert_eq!((f.item, f.section), (123, 0));
        assert_eq!(f.library_db.to_str(), Some(plexdb::DEFAULT_PATH));
    }

    #[test]
    fn inline_values_and_overrides() {
        let f = parse(&[
            "--analyze=true",
            "--section=4",
            "--library-db=/tmp/lib.db",
            "--ffprobe",
            "/bin/ffprobe",
            "--ua=UA/2",
            "--runtime-sock",
            "",
        ])
        .unwrap();
        assert!(f.analyze);
        assert_eq!(f.section, 4);
        assert_eq!(f.library_db.to_str(), Some("/tmp/lib.db"));
        assert_eq!(f.ffprobe, "/bin/ffprobe");
        assert_eq!(f.ua, "UA/2");
        assert_eq!(f.runtime_sock.to_str(), Some(""));
    }

    #[test]
    fn unknown_flag_does_not_swallow_a_flag() {
        let f = parse(&["--scan", "--item", "7", "--refresh=1", "--section", "2"]).unwrap();
        assert!(!f.analyze);
        assert_eq!((f.item, f.section), (7, 2));
    }

    #[test]
    fn known_flags_take_dash_values() {
        let f = parse(&["--analyze", "--item", "-5", "--ua", "-odd-"]).unwrap();
        assert_eq!(f.item, -5);
        assert_eq!(f.ua, "-odd-");
    }

    #[test]
    fn lone_dash_is_positional() {
        let f = parse(&["-", "--item", "3"]).unwrap();
        assert_eq!(f.item, 3);
    }

    #[test]
    fn stops_at_double_dash() {
        let f = parse(&["--analyze=false", "--", "--item", "9"]).unwrap();
        assert!(!f.analyze);
        assert_eq!(f.item, 0);
    }

    #[test]
    fn errors() {
        assert_eq!(
            parse(&["--item", "abc"]).unwrap_err(),
            "invalid argument \"abc\" for --item"
        );
        assert_eq!(
            parse(&["--item"]).unwrap_err(),
            "flag needs an argument: --item"
        );
        assert!(parse(&["--analyze=maybe"]).is_err());
    }
}
