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

/// The exec'd helper carries the bundle's own code-signing identifier.
/// usernoted names a notification client by that identifier; a helper signed
/// under codesign's default (`ducktape-app-<hash>`) is no app it knows, and
/// every notification call answers UNErrorDomain 1 with no consent prompt.
#[test]
fn the_helper_is_signed_with_the_bundle_identifier() {
    let script = std::fs::read_to_string(root().join("ops/bundle-app-macos.sh")).unwrap();
    let helper_sign = script
        .lines()
        .find(|line| {
            line.trim_start().starts_with("codesign") && line.contains("MacOS/ducktape-app\"")
        })
        .expect("the bundle script signs Contents/MacOS/ducktape-app");
    assert!(
        helper_sign.contains("--identifier \"$bundle_id\""),
        "the helper must be signed --identifier <CFBundleIdentifier>: {helper_sign}"
    );
    assert!(
        script.contains("bundle_id=$(/usr/libexec/PlistBuddy -c \"Print :CFBundleIdentifier\""),
        "bundle_id is read from the staged Info.plist, not restated"
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

/// `ops/verify-release.sh` uses one cleaned target dir with reproducibility
/// settings for both builds, fails naming a binary that truly differs, and
/// passes when both builds produce identical bytes.
#[test]
fn verify_release_names_the_binary_two_builds_disagree_on() {
    let dir = tempfile::tempdir().unwrap();
    let cargo = dir.path().join("cargo");
    std::fs::write(
        &cargo,
        r#"#!/bin/sh
printf '%s|%s|%s|%s\n' "$1" "$CARGO_TARGET_DIR" "${RUSTC_WRAPPER-unset}" "${CARGO_INCREMENTAL-unset}" >>"$FAKE_CARGO_LOG"
case $1 in
metadata) printf '{"target_directory": "%s"}\n' "$CARGO_TARGET_DIR" ;;
clean)
  [ "$2" = --target-dir ] && [ "$3" = "$FAKE_TARGET" ] || exit 2
  ;;
build)
  [ "${RUSTC_WRAPPER-}" = "" ] || exit 3
  [ "${CARGO_INCREMENTAL-}" = 0 ] || exit 4
  mkdir -p "$CARGO_TARGET_DIR/release"
  echo launcher >"$CARGO_TARGET_DIR/release/ducktape-launcher"
  builds=$(awk -F'|' '$1 == "build" { n++ } END { print n + 0 }' "$FAKE_CARGO_LOG")
  if [ "${APP_BYTES:-}" = drift ]; then
    echo "app-$builds" >"$CARGO_TARGET_DIR/release/ducktape-app"
  else
    echo "${APP_BYTES:-app}" >"$CARGO_TARGET_DIR/release/ducktape-app"
  fi ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let verify = |name: &str, app_bytes: &str| {
        let target = dir.path().join(name).join("work/target");
        let log = dir.path().join(name).join("cargo.log");
        Command::new(root().join("ops/verify-release.sh"))
            .env("CARGO", &cargo)
            .env("CARGO_INCREMENTAL", "1")
            .env("RUSTC_WRAPPER", "caller-wrapper")
            .env("APP_BYTES", app_bytes)
            .env("FAKE_CARGO_LOG", &log)
            .env("FAKE_TARGET", &target)
            .arg("--work")
            .arg(dir.path().join(name).join("work"))
            .arg(dir.path().join(name).join("out"))
            .output()
            .unwrap()
    };

    let drifting = verify("drifting", "drift");
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

    for name in ["drifting", "steady"] {
        let target = dir.path().join(name).join("work/target");
        let log = std::fs::read_to_string(dir.path().join(name).join("cargo.log")).unwrap();
        let calls: Vec<Vec<&str>> = log.lines().map(|line| line.split('|').collect()).collect();
        assert_eq!(calls.len(), 5, "{name}: {log}");
        assert_eq!(
            calls.iter().map(|call| call[0]).collect::<Vec<_>>(),
            ["metadata", "build", "clean", "metadata", "build"]
        );
        assert!(
            calls.iter().all(|call| call[1] == target.to_str().unwrap()),
            "{name}: {log}"
        );
        assert!(calls.iter().all(|call| call[2].is_empty()), "{name}: {log}");
        assert!(calls.iter().all(|call| call[3] == "0"), "{name}: {log}");
        assert!(dir.path().join(name).join("out/ducktape-app").is_file());
        assert!(
            dir.path()
                .join(name)
                .join("work/stage-2/ducktape-app")
                .is_file()
        );
    }
}
