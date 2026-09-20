#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use app_update::{Idle, Phase, Sha, Staged, state};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_ducktape-launcher");
const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIdentifier</key><string>dev.ducktape.test</string>
<key>CFBundleExecutable</key><string>ducktape-launcher</string>
</dict></plist>
"#;

struct Rig {
    root: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn source(&self, name: &str, quarantined: bool) -> PathBuf {
        let bundle = self.root.path().join(format!("{name}-source/Ducktape.app"));
        let contents = bundle.join("Contents");
        let macos = contents.join("MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::write(contents.join("Info.plist"), INFO_PLIST).unwrap();
        for executable in ["ducktape-app", "ducktape-launcher"] {
            fs::copy(LAUNCHER, macos.join(executable)).unwrap();
            fs::set_permissions(macos.join(executable), fs::Permissions::from_mode(0o755)).unwrap();
        }
        codesign(&macos.join("ducktape-app"));
        codesign(&bundle);
        if quarantined {
            xattr_write(&bundle, "com.apple.quarantine", "0081;00000000;Safari;");
            xattr_write(
                &macos.join("ducktape-app"),
                "com.apple.quarantine",
                "0081;00000000;Safari;",
            );
        }
        xattr_write(&macos.join("ducktape-app"), "com.example.test", "kept");
        bundle
    }

    fn install(&self, source: &Path, name: &str) -> Output {
        let install = self.root.path().join(format!("{name}-install"));
        fs::create_dir_all(&install).unwrap();
        let config = self.root.path().join(format!("{name}-config"));
        let home = self.root.path().join(format!("{name}-home"));
        fs::create_dir_all(&home).unwrap();
        Command::new(LAUNCHER)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", config)
            .env("DUCKTAPE_INSTALL_DIR", &install)
            .args(["install", "--from"])
            .arg(source)
            .output()
            .unwrap()
    }
}

fn codesign(path: &Path) {
    let status = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "codesign failed for {}", path.display());
}

fn xattr_write(path: &Path, name: &str, value: &str) {
    let status = Command::new("/usr/bin/xattr")
        .args(["-w", name, value])
        .arg(path)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "xattr write failed for {}",
        path.display()
    );
}

fn has_xattr(path: &Path, name: &str) -> bool {
    let output = Command::new("/usr/bin/xattr")
        .args(["-lr"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "xattr list failed for {}",
        path.display()
    );
    String::from_utf8_lossy(&output.stdout).contains(name)
}

fn verify_bundle(path: &Path) {
    let status = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(path)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "codesign verify failed for {}",
        path.display()
    );
}

#[test]
fn install_clears_nested_quarantine_but_keeps_code_and_other_attributes() {
    let rig = Rig::new();
    for quarantined in [false, true] {
        let name = if quarantined { "quarantined" } else { "plain" };
        let source = rig.source(name, quarantined);
        let installed = rig.install(&source, name);
        assert!(
            installed.status.success(),
            "install failed for {name}: {}",
            String::from_utf8_lossy(&installed.stderr)
        );
        let bundle = rig.root.path().join(format!("{name}-install/Ducktape.app"));
        verify_bundle(&bundle);
        assert!(!has_xattr(&bundle, "com.apple.quarantine"));
        assert!(has_xattr(
            &bundle.join("Contents/MacOS/ducktape-app"),
            "com.example.test"
        ));
        assert_eq!(has_xattr(&source, "com.apple.quarantine"), quarantined);
    }
}

/// An install is not an update: a second one over the first lays down a clean
/// set. It asks nothing, keeps no bundle to roll back to, and leaves the
/// update root holding only a fresh state — whatever the update path had
/// staged or journalled there. `make install` run twice is this, and it used
/// to refuse the second time.
#[test]
fn a_second_install_leaves_a_clean_set() {
    let rig = Rig::new();
    let first = rig.source("first", false);
    assert!(rig.install(&first, "clean").status.success());

    let updates = rig.root.path().join("clean-config/ducktape/updates");
    let leftover = updates
        .join("releases")
        .join(Sha::digest(b"staged").to_string());
    fs::create_dir_all(leftover.join("Ducktape.app")).unwrap();
    std::os::unix::fs::symlink(&leftover, updates.join("previous")).unwrap();
    fs::write(updates.join("swap.json"), "{}").unwrap();

    // a different build: its executable's digest is what names a local release.
    let second = rig.source("second", false);
    let app = second.join("Contents/MacOS/ducktape-app");
    fs::write(&app, "#!/bin/sh\necho second\n").unwrap();
    codesign(&app);
    codesign(&second);
    let sha = Sha::digest(&fs::read(&app).unwrap());

    let installed = rig.install(&second, "clean");
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let bundle = rig.root.path().join("clean-install/Ducktape.app");
    assert_eq!(
        fs::read(bundle.join("Contents/MacOS/ducktape-app")).unwrap(),
        fs::read(&app).unwrap()
    );
    let state = fs::read_to_string(updates.join("state.json")).unwrap();
    assert_eq!(
        state::decode(&state).unwrap(),
        Phase::Idle(Idle {
            current: sha,
            previous: None,
            pinned_sequence: 0,
        })
    );
    assert!(fs::symlink_metadata(updates.join("previous")).is_err());
    assert!(!updates.join("swap.json").exists());
    assert_eq!(fs::read_dir(updates.join("releases")).unwrap().count(), 0);
    let install_dir: Vec<_> = fs::read_dir(rig.root.path().join("clean-install"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(install_dir, ["Ducktape.app"], "a bundle was left aside");
}

#[test]
fn qualify_accepts_an_adhoc_bundle_without_gatekeeper_assessment() {
    let rig = Rig::new();
    let source = rig.source("qualify", false);
    let config = rig.root.path().join("qualify-config");
    let home = rig.root.path().join("qualify-home");
    fs::create_dir_all(&home).unwrap();
    let updates = config.join("ducktape/updates");
    let staged_sha = Sha::digest(b"staged");
    let release = updates.join("releases").join(staged_sha.to_string());
    let bundle = release.join("Ducktape.app");
    fs::create_dir_all(&release).unwrap();
    let copied = Command::new("/usr/bin/ditto")
        .arg(&source)
        .arg(&bundle)
        .status()
        .unwrap();
    assert!(copied.success());
    let phase = Phase::Staged(Staged {
        current: Sha::digest(b"current"),
        previous: None,
        pinned_sequence: 1,
        staged: staged_sha,
        sequence: 1,
        display: "test".into(),
        node_contract: 1,
        refused: None,
    });
    let state_path = updates.join("state.json");
    fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    fs::write(&state_path, state::encode(&phase)).unwrap();
    let output = Command::new(bundle.join("Contents/MacOS/ducktape-launcher"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", config)
        .arg("--qualify")
        .arg(&state_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "qualify failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok");
}
