//! Fills in a strm item when its detail page is opened.
//!
//! Plex only asks the scanner to analyse while scanning. Opening an item
//! writes a `GET /library/metadata/N` line to the server log and does not
//! spawn the scanner, so an item Plex already considers analysed stays
//! empty. Tailing that log and analysing the item ourselves covers the
//! gap without taking over the port.

use std::path::{Path, PathBuf};

use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader, SeekFrom};
use tokio::time::{Duration, sleep};

/// Where Plex writes the server log inside the container.
pub const DEFAULT_LOG: &str =
    "/config/Library/Application Support/Plex Media Server/Logs/Plex Media Server.log";

/// How often a missing or idle log is polled.
const POLL: Duration = Duration::from_secs(2);

/// The metadata id of a detail-page request, if the line is one.
///
/// A detail page is `GET /library/metadata/N` with a numeric id and no
/// further path. Children, extras, themes and related requests are not
/// the page the user opened, and analysing them would probe a whole
/// season for one click.
pub fn detail_item(line: &str) -> Option<i64> {
    let rest = line.split("GET /library/metadata/").nth(1)?;
    let path = rest.split(|c: char| c.is_whitespace() || c == '?').next()?;
    if path.is_empty() || path.contains('/') {
        return None;
    }
    path.parse().ok()
}

/// Reads new lines from the Plex log and analyses each detail page once.
///
/// The file is followed from the end, so a restart does not re-analyse
/// everything that was opened before. A rotated log (the file shrinking)
/// is read from the start of the new one.
pub async fn follow(log: &Path, analyze: impl Fn(i64)) -> std::io::Result<()> {
    let mut offset = 0u64;
    let mut seen = std::collections::HashSet::new();
    loop {
        match File::open(log).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                sleep(POLL).await;
                continue;
            }
            Err(e) => return Err(e),
            Ok(file) => {
                let len = file.metadata().await?.len();
                let mut file = file;
                if len < offset {
                    offset = 0;
                    seen.clear();
                }
                if offset == 0 {
                    offset = len;
                }
                file.seek(SeekFrom::Start(offset)).await?;
                let mut reader = BufReader::new(file);
                let mut line = String::new();
                loop {
                    line.clear();
                    let n = reader.read_line(&mut line).await?;
                    if n == 0 {
                        break;
                    }
                    offset += n as u64;
                    if let Some(item) = detail_item(line.trim_end())
                        && seen.insert(item)
                    {
                        analyze(item);
                    }
                }
                sleep(POLL).await;
            }
        }
    }
}

/// The log path, overridable so a run outside the container can point at
/// its own copy.
pub fn log_path() -> PathBuf {
    std::env::var("ASSISTANT_PLEX_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_LOG))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detail_request_yields_the_item() {
        let line = "Nov 03, 2026 12:07:28.408 DEBUG - Request: [192.168.148.1:57056 (Subnet)] GET /library/metadata/1023?includeMarkers=1 (5 live) #20a Signed-in Token (AaronYes)";
        assert_eq!(detail_item(line), Some(1023));
    }

    #[test]
    fn a_child_request_is_not_a_detail_page() {
        let line = "GET /library/metadata/1013/children?excludeAllLeaves=1 (6 live)";
        assert_eq!(detail_item(line), None);
    }

    #[test]
    fn a_theme_request_is_not_a_detail_page() {
        let line = "GET /library/metadata/1012/theme/1793609404?Accept-Language=zh";
        assert_eq!(detail_item(line), None);
    }

    #[test]
    fn a_line_without_the_request_yields_nothing() {
        assert_eq!(detail_item("Nov 03, 2026 DEBUG - nothing here"), None);
        assert_eq!(detail_item("GET /library/metadata/?x=1"), None);
    }

    #[tokio::test]
    async fn new_lines_are_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Plex Media Server.log");
        std::fs::write(&path, "GET /library/metadata/7?includeMarkers=1\n").unwrap();
        let path2 = path.clone();
        let found = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let found2 = found.clone();
        let task = tokio::spawn(async move {
            let _ = follow(&path2, |item| found2.lock().unwrap().push(item)).await;
        });
        sleep(Duration::from_millis(50)).await;
        // the line that was already there is not reported
        assert!(found.lock().unwrap().is_empty());
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "GET /library/metadata/9?includeMarkers=1").unwrap();
        writeln!(file, "GET /library/metadata/9/children").unwrap();
        writeln!(file, "GET /library/metadata/9?includeMarkers=1").unwrap();
        sleep(Duration::from_millis(2500)).await;
        assert_eq!(*found.lock().unwrap(), vec![9]);
        task.abort();
    }
}
