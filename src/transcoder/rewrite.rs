use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::path::PathBuf;

use crate::linkres;
use crate::runtime::Client;
use crate::scanner::is_strm;

/// Supplies direct links; None when no link could be had.
pub trait Resolver {
    fn resolve(&self, target: &str) -> impl Future<Output = Option<String>> + Send;
    /// Forces a fresh link (playback hit a dead one).
    fn refresh(&self, target: &str) -> impl Future<Output = Option<String>> + Send;
}

/// Asks the runtime daemon and falls back to resolving directly when it
/// is unreachable.
#[derive(Debug, Clone)]
pub struct DaemonResolver {
    pub sock: Option<PathBuf>,
    pub user_agent: String,
}

impl DaemonResolver {
    async fn ask_daemon(&self, target: &str, refresh: bool) -> Option<String> {
        let client = Client::new(self.sock.as_ref()?).ok()?;
        let res = if refresh {
            client.refresh(target).await
        } else {
            client.resolve(target).await
        };
        res.ok().map(|r| r.direct)
    }

    async fn ask(&self, target: &str, refresh: bool) -> Option<String> {
        if let Some(direct) = self.ask_daemon(target, refresh).await {
            return Some(direct);
        }
        linkres::resolve(target, &self.user_agent)
            .await
            .ok()
            .map(|link| link.url)
    }
}

impl Resolver for DaemonResolver {
    async fn resolve(&self, target: &str) -> Option<String> {
        self.ask(target, false).await
    }

    async fn refresh(&self, target: &str) -> Option<String> {
        self.ask(target, true).await
    }
}

/// One argv input replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swap {
    /// strm URL
    pub target: String,
    /// link placed into argv
    pub direct: String,
    /// argv position of the replaced input
    pub arg_index: usize,
}

/// Replaces every "-i <path>.strm" input with a resolved direct link and
/// injects the resolving user agent before it: cloud CDNs bind links to
/// the UA that resolved them, and the original transcoder would otherwise
/// fetch with ffmpeg's default one. Arguments stay OsStrings because Plex
/// paths need not be UTF-8.
pub async fn rewrite_args(
    args: &[OsString],
    res: &impl Resolver,
    ua: &str,
) -> (Vec<OsString>, Vec<Swap>) {
    let mut out = Vec::with_capacity(args.len() + 4);
    let mut swaps = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-i"
            && let Some(input) = args.get(i + 1)
            && let Some(swap) = swap_input(input, res, out.len() + 3).await
        {
            out.extend(["-user_agent", ua, "-i", &swap.direct].map(OsString::from));
            swaps.push(swap);
            i += 2; // consumed the strm path too
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    (out, swaps)
}

/// Resolves a strm input to a direct link that will land at arg_index.
async fn swap_input(input: &OsStr, res: &impl Resolver, arg_index: usize) -> Option<Swap> {
    if !is_strm(&input.to_string_lossy()) {
        return None;
    }
    let target = linkres::parse_strm(input).ok().filter(|t| t.is_remote)?;
    let direct = res.resolve(&target.value).await.filter(|d| !d.is_empty())?;
    Some(Swap {
        target: target.value,
        direct,
        arg_index,
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    pub(in crate::transcoder) struct FakeResolver {
        /// target -> direct
        pub links: HashMap<String, String>,
        /// target -> refreshed link
        pub refreshed: HashMap<String, String>,
        pub resolves: AtomicUsize,
    }

    impl Resolver for FakeResolver {
        async fn resolve(&self, target: &str) -> Option<String> {
            self.resolves.fetch_add(1, Ordering::SeqCst);
            self.links.get(target).cloned()
        }

        async fn refresh(&self, target: &str) -> Option<String> {
            self.refreshed.get(target).cloned()
        }
    }

    pub(in crate::transcoder) fn write_strm(path: &std::path::Path, url: &str) {
        std::fs::write(path, format!("{url}\n")).unwrap();
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[tokio::test]
    async fn rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let strm_a = dir.path().join("a.strm");
        let strm_b = dir.path().join("b.strm");
        let local = dir.path().join("c.mkv");
        write_strm(&strm_a, "http://x/file/ep1");
        write_strm(&strm_b, "/local/path.mkv"); // local target: keep
        std::fs::write(&local, "x").unwrap();
        let (a, b, c) = (
            strm_a.to_str().unwrap(),
            strm_b.to_str().unwrap(),
            local.to_str().unwrap(),
        );

        let res = FakeResolver {
            links: HashMap::from([("http://x/file/ep1".into(), "http://x/dl/fresh".into())]),
            ..Default::default()
        };
        let args = os(&["-codec:0", "h264", "-i", a, "-i", b, "-i", c]);
        let (out, swaps) = rewrite_args(&args, &res, "UA/1").await;

        // swapped input is preceded by the resolving user agent
        assert_eq!(
            out,
            os(&[
                "-codec:0",
                "h264",
                "-user_agent",
                "UA/1",
                "-i",
                "http://x/dl/fresh",
                "-i",
                b,
                "-i",
                c
            ])
        );
        assert_eq!(
            swaps,
            [Swap {
                target: "http://x/file/ep1".into(),
                direct: "http://x/dl/fresh".into(),
                arg_index: 5,
            }]
        );
    }

    #[tokio::test]
    async fn unresolvable_and_edge_inputs_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let strm = dir.path().join("A.STRM");
        write_strm(&strm, "http://x/file/unknown");
        let s = strm.to_str().unwrap();
        let res = FakeResolver::default();

        // resolver has no link; trailing "-i" has no value
        let args = os(&["-i", s, "-i"]);
        let (out, swaps) = rewrite_args(&args, &res, "UA").await;
        assert_eq!(out, args);
        assert!(swaps.is_empty());
        assert_eq!(
            res.resolves.load(Ordering::SeqCst),
            1,
            "upper-case suffix still tried"
        );
    }

    #[tokio::test]
    async fn non_utf8_args_pass_through() {
        use std::os::unix::ffi::OsStringExt;
        let odd = OsString::from_vec(vec![b'/', 0xff, b'.', b'm', b'k', b'v']);
        let args = vec![OsString::from("-i"), odd];
        let (out, swaps) = rewrite_args(&args, &FakeResolver::default(), "UA").await;
        assert_eq!(out, args);
        assert!(swaps.is_empty());
    }

    #[tokio::test]
    async fn daemon_resolver_falls_back_to_direct() {
        use axum::Router;
        use axum::http::{StatusCode, header};
        use axum::routing::get;

        let app = Router::new()
            .route(
                "/file/ep1",
                get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/dl/ep1")]) }),
            )
            .route("/dl/ep1", get(|| async { "0" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // the daemon socket is missing: both calls resolve upstream instead
        let res = DaemonResolver {
            sock: Some(PathBuf::from("/nonexistent/assistant.sock")),
            user_agent: linkres::DEFAULT_UA.into(),
        };
        let target = format!("{base}/file/ep1");
        let direct = Some(format!("{base}/dl/ep1"));
        assert_eq!(res.resolve(&target).await, direct);
        assert_eq!(res.refresh(&target).await, direct);
        assert_eq!(res.resolve("http://127.0.0.1:1/file/x").await, None);
    }
}
