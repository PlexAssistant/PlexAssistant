//! Checks the s6-overlay services the image copies to /etc/s6-overlay.
//!
//! s6-rc reads its service graph from file names, and a mistake there
//! only shows when the container boots: a typo in a dependency, a script
//! that lost its exec bit, or a socket path that drifted from the code.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use plex_assistant::runtime::DEFAULT_SOCK;

/// Where the image installs the binary (see the Dockerfile).
const BINARY: &str = "/usr/local/bin/assistant";

/// Services the linuxserver base image defines. Ours depend on them, or
/// add dependencies to them, without defining them.
const BASE_SERVICES: &[&str] = &["init-config-end", "init-services", "svc-plex", "user"];

fn overlay() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/s6-overlay")
}

fn services() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(overlay().join("s6-rc.d"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn assert_executable(path: &Path) {
    let mode = fs::metadata(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0, "{} is not executable", path.display());
}

#[test]
fn every_service_has_a_type_and_its_script() {
    for dir in services() {
        let service = name(&dir);
        let type_file = dir.join("type");
        if !type_file.exists() {
            // only adds dependencies to a service the base image defines
            assert!(
                BASE_SERVICES.contains(&service.as_str()),
                "{service} has no type and is not a base service"
            );
            continue;
        }
        match read(&type_file).trim() {
            "longrun" => assert_executable(&dir.join("run")),
            "oneshot" => {
                // `up` is one execline command; ours names a script in the overlay
                let up = read(&dir.join("up"));
                let script = up.split_whitespace().next().expect("empty up");
                let local = script
                    .strip_prefix("/etc/s6-overlay/")
                    .unwrap_or_else(|| panic!("{service}/up runs {script}, outside the overlay"));
                assert_executable(&overlay().join(local));
            }
            "bundle" => {}
            other => panic!("{service}: unknown type {other:?}"),
        }
    }
}

#[test]
fn every_dependency_names_a_known_service() {
    let ours: Vec<String> = services().iter().map(|dir| name(dir)).collect();
    for dir in services() {
        for list in ["dependencies.d", "contents.d"] {
            let Ok(entries) = fs::read_dir(dir.join(list)) else {
                continue;
            };
            for entry in entries {
                let target = entry.unwrap().file_name().to_string_lossy().into_owned();
                assert!(
                    ours.contains(&target) || BASE_SERVICES.contains(&target.as_str()),
                    "{}/{list}/{target} names an unknown service",
                    name(&dir)
                );
            }
        }
    }
}

#[test]
fn plex_starts_after_the_install() {
    // Plex must find our scanner and transcoder in place when it starts
    let rc = overlay().join("s6-rc.d");
    assert!(
        rc.join("svc-plex/dependencies.d/assistant-install")
            .exists()
    );
    assert!(rc.join("user/contents.d/assistant-install").exists());
    assert!(rc.join("user/contents.d/assistant-runtime").exists());
}

#[test]
fn scripts_match_the_image_and_the_code() {
    let rc = overlay().join("s6-rc.d");
    let dockerfile = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("Dockerfile"));
    assert!(
        dockerfile.contains(&format!("COPY --from=build /out/assistant {BINARY}")),
        "the Dockerfile no longer installs the binary at {BINARY}"
    );

    let install = read(&rc.join("assistant-install/install.sh"));
    assert!(install.contains(&format!("exec {BINARY} install")));

    let run = read(&rc.join("assistant-runtime/run"));
    assert!(run.contains(&format!("{BINARY} runtime --sock {DEFAULT_SOCK}")));
    let sock_dir = Path::new(DEFAULT_SOCK)
        .parent()
        .unwrap()
        .display()
        .to_string();
    assert!(
        run.contains(&format!("mkdir -p {sock_dir}")),
        "run must create {sock_dir}"
    );

    // editions is opt-in: the run script must keep the gate and the
    // listener command in step with the cli
    let editions = read(&rc.join("assistant-editions/run"));
    assert!(editions.contains(&format!("{BINARY} editions new")));
    assert!(
        editions.contains("ASSISTANT_EDITIONS"),
        "editions run must stay behind ASSISTANT_EDITIONS"
    );
}
