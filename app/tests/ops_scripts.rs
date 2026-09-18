//! THE BUNDLE IS BUILT FROM THIS REPOSITORY ALONE. `app/README.md` tells a
//! person to run `ops/…` scripts directly, so each one it names is executable
//! as checked out; and every `ops/…` file the bundle script reads is here,
//! not left behind in the repository it was split from. The views it ships
//! are the revision `ops/views.rev` pins, or it refuses them.
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn pin() -> String {
    std::fs::read_to_string(root().join("ops/views.rev")).expect("ops/views.rev")
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

/// The pin is one commit and nothing else, so a script reads it whole.
#[test]
fn the_views_pin_is_one_full_commit() {
    let pin = pin();
    let commit = pin.strip_suffix('\n').expect("one line");
    assert!(
        commit.len() == 40
            && commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "ops/views.rev is not 40 lowercase hex and a newline: {pin:?}"
    );
    let tools = std::fs::read_to_string(root().join("ops/views.wasm-tools")).unwrap();
    let version = tools.strip_suffix('\n').expect("one line");
    assert!(
        version.split('.').count() == 3
            && version.split('.').all(|part| part.parse::<u32>().is_ok()),
        "ops/views.wasm-tools is not one wasm-tools version: {tools:?}"
    );
}

#[test]
fn the_views_scripts_are_executable_and_parse() {
    for script in ["ops/stage-views.sh", "ops/stage-release.sh"] {
        let mode = std::fs::metadata(root().join(script))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "{script} is not executable: {mode:o}");
    }
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

/// Both release scripts refuse a `DUCKTAPE_VIEWS_DIR` whose `VIEWS_REV` is
/// not the pin — a wrong one or none — before anything is built, and name
/// both revisions. `DUCKTAPE_VIEWS_UNPINNED=1` lets it through, loudly.
#[test]
fn a_release_refuses_views_not_built_from_the_pin() {
    let pin = pin();
    let pin = pin.trim_end();
    let scratch = tempfile::tempdir().unwrap();
    let views = scratch.path().join("views");
    std::fs::create_dir(&views).unwrap();
    std::fs::write(views.join("members_view.wasm"), b"\0asm").unwrap();
    let wrong = "0".repeat(40);
    let run = |script: &str, views_rev: Option<&str>, unpinned: bool| {
        let _ = std::fs::remove_file(views.join("VIEWS_REV"));
        let _ = std::fs::remove_dir_all(scratch.path().join("out"));
        if let Some(rev) = views_rev {
            std::fs::write(views.join("VIEWS_REV"), format!("{rev}\n")).unwrap();
        }
        let mut command = Command::new(root().join(script));
        command
            .arg(scratch.path().join("out"))
            .env("DUCKTAPE_VIEWS_DIR", &views)
            .env("CARGO", "false");
        for name in [
            "DUCKTAPE_VIEWS_UNPINNED",
            "DUCKTAPE_SIGN_VIA",
            "DUCKTAPE_CODESIGN_IDENTITY",
            "DUCKTAPE_NOTARY_KEY",
            "DUCKTAPE_NOTARY_KEY_ID",
            "DUCKTAPE_NOTARY_ISSUER",
        ] {
            command.env_remove(name);
        }
        if unpinned {
            command.env("DUCKTAPE_VIEWS_UNPINNED", "1");
        }
        let output = command.output().unwrap();
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };

    for script in ["ops/bundle-app-macos.sh", "ops/stage-release.sh"] {
        for (views_rev, holds) in [
            (Some(wrong.as_str()), format!("holds views {wrong}")),
            (None, "holds views with no VIEWS_REV".to_string()),
        ] {
            let (ok, stderr) = run(script, views_rev, false);
            assert!(!ok, "{script} shipped views {views_rev:?}");
            assert!(
                stderr.contains(&format!(
                    "DUCKTAPE_VIEWS_DIR={} {holds}, not the pinned {pin} (ops/views.rev)",
                    views.display()
                )),
                "{script}: {stderr}"
            );
        }
        // Past the check, the build is `false`: it fails, but after the views
        // were taken.
        for (views_rev, unpinned) in [(Some(pin), false), (Some(wrong.as_str()), true)] {
            let (_, stderr) = run(script, views_rev, unpinned);
            assert!(!stderr.contains("not the pinned"), "{script}: {stderr}");
            assert_eq!(
                stderr.contains("WARNING: DUCKTAPE_VIEWS_UNPINNED=1 ships the views"),
                unpinned,
                "{script}: {stderr}"
            );
        }
    }
}
