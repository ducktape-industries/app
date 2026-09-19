//! THE BUNDLE IS BUILT FROM THIS REPOSITORY ALONE. `app/README.md` tells a
//! person to run `ops/…` scripts directly, so each one it names is executable
//! as checked out; and every `ops/…` file the bundle script reads is here,
//! not left behind in the repository it was split from.
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn the_documented_scripts_run_from_a_clean_checkout() {
    let root = root();
    let read = |path: &str| {
        std::fs::read_to_string(root.join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
    };

    let readme = read("app/README.md");
    let documented: Vec<String> = std::fs::read_dir(root.join("ops"))
        .unwrap()
        .map(|entry| format!("ops/{}", entry.unwrap().file_name().to_string_lossy()))
        .filter(|path| path.ends_with(".sh") && readme.contains(path.as_str()))
        .collect();
    assert!(
        documented
            .iter()
            .any(|path| path == "ops/bundle-app-macos.sh"),
        "the walk did not find ops/bundle-app-macos.sh in app/README.md: {documented:?}"
    );
    let not_executable: Vec<&String> = documented
        .iter()
        .filter(|path| {
            let mode = std::fs::metadata(root.join(path))
                .unwrap()
                .permissions()
                .mode();
            mode & 0o111 == 0
        })
        .collect();
    assert_eq!(
        not_executable,
        [] as [&String; 0],
        "app/README.md runs these directly"
    );

    let script = read("ops/bundle-app-macos.sh");
    let reads: Vec<&str> = script
        .match_indices("ops/")
        .map(|(at, _)| {
            let token = &script[at..];
            let end = token
                .find(|c: char| !(c.is_ascii_alphanumeric() || "._/-".contains(c)))
                .unwrap_or(token.len());
            token[..end].trim_end_matches('.')
        })
        .collect();
    assert!(
        !reads.is_empty(),
        "the walk found no ops/ path in the bundle script"
    );
    let missing: Vec<&str> = reads
        .into_iter()
        .filter(|path| !root.join(path).is_file())
        .collect();
    assert_eq!(
        missing,
        [] as [&str; 0],
        "ops/bundle-app-macos.sh reads files this repository does not hold"
    );
}

/// Every `ops/*.sh` parses: a script only a release runs is not first read
/// by the release.
#[test]
fn every_ops_script_parses() {
    for script in std::fs::read_dir(root().join("ops")).unwrap() {
        let script = script.unwrap().path();
        if script.extension().is_some_and(|ext| ext == "sh") {
            let parsed = Command::new("bash")
                .arg("-n")
                .arg(&script)
                .output()
                .unwrap();
            assert!(
                parsed.status.success(),
                "{}: {}",
                script.display(),
                String::from_utf8_lossy(&parsed.stderr)
            );
        }
    }
}

/// `ops/verify-release.sh` fails naming the binary two stagings disagree on,
/// and passes when they agree. The stand-in cargo builds an app whose bytes
/// follow its target dir (the drift #130 found) unless `APP_BYTES` pins them.
#[test]
fn verify_release_names_the_binary_two_builds_disagree_on() {
    let dir = tempfile::tempdir().unwrap();
    let cargo = dir.path().join("cargo");
    std::fs::write(
        &cargo,
        r#"#!/bin/sh
case $1 in
metadata) printf '{"target_directory": "%s"}\n' "$CARGO_TARGET_DIR" ;;
build)
  mkdir -p "$CARGO_TARGET_DIR/release"
  echo launcher >"$CARGO_TARGET_DIR/release/ducktape-launcher"
  echo "${APP_BYTES:-$CARGO_TARGET_DIR}" >"$CARGO_TARGET_DIR/release/ducktape-app" ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let verify = |name: &str, app_bytes: &str| {
        Command::new(root().join("ops/verify-release.sh"))
            .env("CARGO", &cargo)
            .env("APP_BYTES", app_bytes)
            .arg("--work")
            .arg(dir.path().join(name).join("work"))
            .arg(dir.path().join(name).join("out"))
            .output()
            .unwrap()
    };

    let drifting = verify("drifting", "");
    let stderr = String::from_utf8_lossy(&drifting.stderr);
    assert_eq!(drifting.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("ducktape-app differs"), "{stderr}");
    assert!(!stderr.contains("ducktape-launcher differs"), "{stderr}");

    let steady = verify("steady", "app");
    assert!(
        steady.status.success(),
        "{}",
        String::from_utf8_lossy(&steady.stderr)
    );
    assert!(dir.path().join("steady/out/ducktape-app").is_file());
}
