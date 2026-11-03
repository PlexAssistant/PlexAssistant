use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::signal::unix::{SignalKind, signal};

use super::rewrite::{Resolver, rewrite_args};

/// How quickly the original must fail before we suspect a dead link and
/// retry once with refreshed ones.
const FAST_FAIL_WINDOW: Duration = Duration::from_secs(30);

/// Signals relayed to the original transcoder so Plex can stop or poke it
/// through the wrapper.
const FORWARDED: [i32; 7] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGUSR1,
    libc::SIGUSR2,
    libc::SIGWINCH,
];

/// Rewrites the args, runs the preserved original as a child with signal
/// relaying, and retries once with refreshed links when it dies fast with
/// our swap in place. Returns the final exit code.
pub async fn run(args: &[OsString], real: &Path, res: &impl Resolver, ua: &str) -> i32 {
    run_within(args, real, res, ua, FAST_FAIL_WINDOW).await
}

async fn run_within(
    args: &[OsString],
    real: &Path,
    res: &impl Resolver,
    ua: &str,
    fast_fail: Duration,
) -> i32 {
    let (mut rewritten, swaps) = rewrite_args(args, res, ua).await;

    let start = Instant::now();
    let code = run_child(real, &rewritten).await;
    if code == 0 || swaps.is_empty() || start.elapsed() > fast_fail {
        return code;
    }

    // Fast failure with our link in argv: probably dead. Refresh every
    // swapped target and retry once: the playback-time self-heal path.
    let mut refreshed = false;
    for swap in &swaps {
        match res.refresh_at(&swap.target, swap.seek).await {
            Some(fresh) if !fresh.is_empty() && fresh != swap.direct => {
                rewritten[swap.arg_index] = fresh.into();
                refreshed = true;
            }
            _ => {}
        }
    }
    if !refreshed {
        // not even a different link could be had: book them dead
        for swap in &swaps {
            res.dead(&swap.target).await;
        }
        return code;
    }
    let code = run_child(real, &rewritten).await;
    if code != 0 {
        for swap in &swaps {
            res.dead(&swap.target).await;
        }
    }
    code
}

/// Starts the original, relays signals to it, waits and returns its exit
/// code (128 + signal when it was killed by one).
async fn run_child(real: &Path, args: &[OsString]) -> i32 {
    // listen before spawning so no signal slips through the gap
    let streams: Vec<_> = FORWARDED
        .iter()
        .filter_map(|&sig| Some((sig, signal(SignalKind::from_raw(sig)).ok()?)))
        .collect();

    let mut child = match Command::new(real).args(args).stdin(Stdio::null()).spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("transcoder wrap: {e}");
            return 1;
        }
    };
    let Some(pid) = child.id().and_then(|id| i32::try_from(id).ok()) else {
        return 1;
    };
    let relays: Vec<_> = streams
        .into_iter()
        .map(|(sig, mut stream)| {
            tokio::spawn(async move {
                while stream.recv().await.is_some() {
                    // SAFETY: kill(2) has no memory-safety preconditions.
                    unsafe { libc::kill(pid, sig) };
                }
            })
        })
        .collect();

    let status = child.wait().await;
    for relay in relays {
        relay.abort();
    }
    match status {
        Ok(status) => status
            .code()
            .or_else(|| status.signal().map(|sig| 128 + sig))
            .unwrap_or(1),
        Err(e) => {
            eprintln!("transcoder wrap: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcoder::rewrite::tests::{FakeResolver, write_strm};
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;
    use tokio::sync::Mutex;

    /// The signal test signals the whole test process; keep every test
    /// that runs children out of its way.
    static CHILDREN: Mutex<()> = Mutex::const_new(());

    fn script(dir: &tempfile::TempDir, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Fake original binary: fails when an arg contains DEADLINK.
    fn fake_real(dir: &tempfile::TempDir) -> std::path::PathBuf {
        script(
            dir,
            "real",
            r#"for a in "$@"; do case "$a" in *DEADLINK*) exit 9;; esac; done
exit 0"#,
        )
    }

    fn dead_then_good() -> FakeResolver {
        FakeResolver {
            links: HashMap::from([("http://x/file/ep1".into(), "http://DEADLINK/old".into())]),
            refreshed: HashMap::from([("http://x/file/ep1".into(), "http://x/dl/good".into())]),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn retries_on_dead_link() {
        let _guard = CHILDREN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let strm = dir.path().join("a.strm");
        write_strm(&strm, "http://x/file/ep1");
        let res = dead_then_good();

        let args = ["-i".into(), strm.into_os_string()];
        let code = run(&args, &fake_real(&dir), &res, "UA/1").await;
        assert_eq!(code, 0, "exit after refresh retry");
        assert_eq!(res.resolves.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn no_swaps_no_retry() {
        let _guard = CHILDREN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("c.mkv");
        std::fs::write(&local, "x").unwrap();
        let args = ["-i".into(), local.into_os_string(), "DEADLINK".into()];
        let code = run(&args, &fake_real(&dir), &FakeResolver::default(), "UA/1").await;
        assert_eq!(code, 9, "no retry without swap");
    }

    #[tokio::test]
    async fn slow_failure_is_not_retried() {
        let _guard = CHILDREN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let strm = dir.path().join("a.strm");
        write_strm(&strm, "http://x/file/ep1");
        let args = ["-i".into(), strm.into_os_string()];
        let code = run_within(
            &args,
            &fake_real(&dir),
            &dead_then_good(),
            "UA",
            Duration::ZERO,
        )
        .await;
        assert_eq!(code, 9, "failure outside the window keeps its code");
    }

    #[tokio::test]
    async fn unchanged_refresh_is_not_retried() {
        let _guard = CHILDREN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let strm = dir.path().join("a.strm");
        write_strm(&strm, "http://x/file/ep1");
        let res = FakeResolver {
            links: HashMap::from([("http://x/file/ep1".into(), "http://DEADLINK/old".into())]),
            refreshed: HashMap::from([("http://x/file/ep1".into(), "http://DEADLINK/old".into())]),
            ..Default::default()
        };
        let args = ["-i".into(), strm.into_os_string()];
        assert_eq!(run(&args, &fake_real(&dir), &res, "UA").await, 9);
    }

    #[tokio::test]
    async fn exit_codes() {
        let _guard = CHILDREN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let killed = script(&dir, "killed", "kill -KILL $$");
        assert_eq!(run_child(&killed, &[]).await, 128 + libc::SIGKILL);
        assert_eq!(run_child(&dir.path().join("missing"), &[]).await, 1);
    }

    #[tokio::test]
    async fn forwards_signals() {
        let _guard = CHILDREN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let got = dir.path().join("got");
        let ready = dir.path().join("ready");
        let child = script(
            &dir,
            "trapper",
            &format!(
                "trap 'echo TERM > \"{}\"; exit 0' TERM\ntouch \"{}\"\nwhile :; do sleep 0.05; done",
                got.display(),
                ready.display()
            ),
        );
        let run = tokio::spawn(async move { run_child(&child, &[]).await });
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // SAFETY: kill(2) has no memory-safety preconditions.
        unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };

        let code = tokio::time::timeout(Duration::from_secs(10), run)
            .await
            .expect("child must stop after the relayed SIGTERM")
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(std::fs::read_to_string(got).unwrap().trim(), "TERM");
    }
}
