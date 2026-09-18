//! The launcher, end to end, on a scratch XDG root: the real binary is
//! spawned for every mode and `ducktape-app` is a shell script that prints
//! what the launcher handed it. Every assertion reads the process's exit
//! and output — nothing waits on time.

#![cfg(target_os = "linux")]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use app_update::{Idle, Phase, RollbackReason, Sha, Staged, state};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_ducktape-launcher");

struct Rig {
    _root: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
}

impl Rig {
    fn new() -> Rig {
        let root = tempfile::tempdir().unwrap();
        let rig = Rig {
            home: root.path().join("home"),
            config: root.path().join("cfg"),
            // a space, so every flow runs where an unquoted `Exec` splits.
            data: root.path().join("local share"),
            _root: root,
        };
        fs::create_dir_all(&rig.home).unwrap();
        rig
    }

    fn install_dir(&self) -> PathBuf {
        self.data.join("ducktape")
    }

    fn state_path(&self) -> PathBuf {
        self.config
            .join("ducktape")
            .join("updates")
            .join("state.json")
    }

    fn release_dir(&self, sha: Sha) -> PathBuf {
        self.install_dir().join("releases").join(sha.to_string())
    }

    /// A release source: `ducktape-app` is a script that prints its pid, env
    /// and argv, tagged with `build`.
    fn build(&self, build: &str) -> (PathBuf, Sha) {
        let dir = self.home.join(format!("build-{build}"));
        fs::create_dir_all(&dir).unwrap();
        let script = format!(
            "#!/bin/sh\necho build={build}\necho release=$DUCKTAPE_RELEASE\necho state=$DUCKTAPE_UPDATE_STATE\necho pid=$$\nfor a in \"$@\"; do echo arg=$a; done\n"
        );
        let app = dir.join("ducktape-app");
        fs::write(&app, &script).unwrap();
        fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
        (dir, Sha::digest(script.as_bytes()))
    }

    /// Stage a release the way the app would: a complete `releases/<sha>`,
    /// or — `with_app` false — one missing its app.
    fn stage(&self, build: &str, with_app: bool) -> Sha {
        let (source, sha) = self.build(build);
        let dir = self.release_dir(sha);
        fs::create_dir_all(&dir).unwrap();
        if with_app {
            fs::copy(source.join("ducktape-app"), dir.join("ducktape-app")).unwrap();
        }
        fs::copy(LAUNCHER, dir.join("ducktape-launcher")).unwrap();
        sha
    }

    fn command(&self, exe: &Path) -> Command {
        let mut command = Command::new(exe);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_DATA_HOME", &self.data)
            .env("RUST_LOG", "info,ducktape::update=debug");
        command
    }

    fn launcher(&self, args: &[&str]) -> Output {
        self.command(Path::new(LAUNCHER))
            .args(args)
            .output()
            .unwrap()
    }

    /// The installed launcher, as the desktop entry runs it.
    fn installed_launcher(&self, args: &[&str]) -> Output {
        let exe = self.install_dir().join("current").join("ducktape-launcher");
        self.command(&exe).args(args).output().unwrap()
    }

    fn read_state(&self) -> Phase {
        state::decode(&fs::read_to_string(self.state_path()).unwrap()).unwrap()
    }

    fn write_state(&self, phase: &Phase) {
        fs::write(self.state_path(), state::encode(phase)).unwrap();
    }

    fn release_key_path(&self) -> PathBuf {
        self.config
            .join("ducktape")
            .join("updates")
            .join("keys")
            .join("release.pub")
    }

    fn link(&self, name: &str) -> Option<PathBuf> {
        fs::read_link(self.install_dir().join(name)).ok()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn link_target(sha: Sha) -> PathBuf {
    PathBuf::from("releases").join(sha.to_string())
}

fn staged(current: Sha, staged: Sha) -> Phase {
    Phase::Staged(Staged {
        current,
        previous: None,
        pinned_sequence: 1,
        staged,
        sequence: 1,
        display: "test".into(),
        node_contract: 1,
        refused: None,
    })
}

#[test]
fn install_boot_update_crash_and_rollback() {
    let rig = Rig::new();
    let (source_a, sha_a) = rig.build("A");

    // install: seed, flip, state, desktop entry; the source carried no
    // launcher, so the installing one was copied in.
    let installed = rig.launcher(&["install", "--from", source_a.to_str().unwrap()]);
    assert!(installed.status.success(), "{}", stderr(&installed));
    assert!(stdout(&installed).contains(&format!("installed {sha_a}")));
    assert_eq!(rig.link("current"), Some(link_target(sha_a)));
    assert_eq!(rig.link("previous"), None);
    assert_eq!(
        rig.read_state(),
        Phase::Idle(Idle {
            current: sha_a,
            previous: None,
            pinned_sequence: 0,
        })
    );
    let launcher_mode = fs::metadata(rig.release_dir(sha_a).join("ducktape-launcher"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(launcher_mode & 0o111, 0o111);
    let entry = fs::read_to_string(
        rig.data
            .join("applications")
            .join("dev.ducktape.app.desktop"),
    )
    .unwrap();
    assert!(
        entry.contains(&format!(
            "Exec=\"{}\" %u",
            rig.install_dir()
                .join("current/ducktape-launcher")
                .display()
        )),
        "{entry}"
    );
    assert!(entry.contains("StartupWMClass=dev.ducktape.app"));

    // boot in Idle: exec A with the contract env and argv passed through;
    // the URL never reaches the launcher's own log.
    let url = "duck://forge/ducktape/1?net=abc";
    let booted = rig.installed_launcher(&[url]);
    assert!(booted.status.success(), "{}", stderr(&booted));
    let out = stdout(&booted);
    assert!(out.contains("build=A"), "{out}");
    assert!(out.contains(&format!("release={sha_a}")), "{out}");
    assert!(
        out.contains(&format!("state={}", rig.state_path().display())),
        "{out}"
    );
    assert!(out.contains(&format!("arg={url}")), "{out}");
    assert!(!stderr(&booted).contains("duck://"), "{}", stderr(&booted));

    // a staged release: boot qualifies it with its own launcher, flips,
    // persists PendingHealthy{boots: 0} and execs it.
    let sha_b = rig.stage("B", true);
    rig.write_state(&staged(sha_a, sha_b));
    let flipped = rig.installed_launcher(&[]);
    assert!(flipped.status.success(), "{}", stderr(&flipped));
    assert!(stdout(&flipped).contains("build=B"), "{}", stdout(&flipped));
    assert!(stdout(&flipped).contains(&format!("release={sha_b}")));
    assert!(
        stderr(&flipped).contains("app_update_flipped"),
        "{}",
        stderr(&flipped)
    );
    assert_eq!(rig.link("current"), Some(link_target(sha_b)));
    assert_eq!(rig.link("previous"), Some(link_target(sha_a)));
    let Phase::PendingHealthy(pending) = rig.read_state() else {
        panic!("{:?}", rig.read_state());
    };
    assert_eq!(
        (pending.current, pending.previous, pending.boots),
        (sha_b, sha_a, 0)
    );

    // the app never rendered: the next boot counts, the one after rolls back.
    let counted = rig.installed_launcher(&[]);
    assert!(counted.status.success(), "{}", stderr(&counted));
    assert!(stdout(&counted).contains("build=B"));
    let Phase::PendingHealthy(pending) = rig.read_state() else {
        panic!("{:?}", rig.read_state());
    };
    assert_eq!(pending.boots, 1);

    let rolled = rig.installed_launcher(&[]);
    assert!(rolled.status.success(), "{}", stderr(&rolled));
    assert!(stdout(&rolled).contains("build=A"), "{}", stdout(&rolled));
    assert!(stdout(&rolled).contains(&format!("release={sha_a}")));
    assert_eq!(rig.link("current"), Some(link_target(sha_a)));
    assert_eq!(rig.link("previous"), Some(link_target(sha_b)));
    let Phase::RolledBack(rolled_back) = rig.read_state() else {
        panic!("{:?}", rig.read_state());
    };
    assert_eq!(
        (rolled_back.current, rolled_back.failed, rolled_back.reason),
        (sha_a, sha_b, RollbackReason::NeverRendered)
    );

    // --rollback from Idle with a previous: flip to it and exec it.
    rig.write_state(&Phase::Idle(Idle {
        current: sha_a,
        previous: Some(sha_b),
        pinned_sequence: 1,
    }));
    let manual = rig.installed_launcher(&["--rollback"]);
    assert!(manual.status.success(), "{}", stderr(&manual));
    assert!(stdout(&manual).contains("build=B"), "{}", stdout(&manual));
    // the event names both sides: rolled back FROM a, TO b.
    let rolled_back_line = stderr(&manual)
        .lines()
        .find(|line| line.contains("app_update_rolled_back"))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("{}", stderr(&manual)));
    assert!(
        rolled_back_line.contains(&sha_a.to_string())
            && rolled_back_line.contains(&sha_b.to_string()),
        "{rolled_back_line}"
    );
    assert_eq!(rig.link("current"), Some(link_target(sha_b)));
    let Phase::PendingHealthy(pending) = rig.read_state() else {
        panic!("{:?}", rig.read_state());
    };
    assert_eq!((pending.current, pending.previous), (sha_b, sha_a));

    // --rollback with nothing to roll back to is a refusal, not a boot.
    rig.write_state(&Phase::Idle(Idle {
        current: sha_b,
        previous: None,
        pinned_sequence: 1,
    }));
    let refused = rig.installed_launcher(&["--rollback"]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("rollback_unavailable"),
        "{}",
        stderr(&refused)
    );
    assert!(!stdout(&refused).contains("build="));
}

#[test]
fn a_staged_release_that_fails_to_qualify_stays_staged_and_the_current_one_runs() {
    let rig = Rig::new();
    let (source_a, sha_a) = rig.build("A");
    assert!(
        rig.launcher(&["install", "--from", source_a.to_str().unwrap()])
            .status
            .success()
    );
    let sha_c = rig.stage("C", false);
    rig.write_state(&staged(sha_a, sha_c));
    let booted = rig.installed_launcher(&[]);
    assert!(booted.status.success(), "{}", stderr(&booted));
    assert!(stdout(&booted).contains("build=A"), "{}", stdout(&booted));
    assert!(stdout(&booted).contains(&format!("release={sha_a}")));
    let err = stderr(&booted);
    assert!(err.contains("app_update_refused"), "{err}");
    assert!(err.contains("executable_missing"), "{err}");
    // the reason is persisted, so the app that comes up can say why (#57).
    let Phase::Staged(after) = rig.read_state() else {
        panic!("{:?}", rig.read_state());
    };
    assert_eq!(after.refused.as_deref(), Some("executable_missing"));
    let unrefused = Staged {
        refused: None,
        ..after
    };
    assert_eq!(Phase::Staged(unrefused), staged(sha_a, sha_c));
    assert_eq!(rig.link("current"), Some(link_target(sha_a)));

    // --rollback does not discard it (collecting its directory is the
    // running app's): a refusal that says where, nothing touched.
    let before = rig.read_state();
    let rollback = rig.installed_launcher(&["--rollback"]);
    assert!(!rollback.status.success());
    let err = stderr(&rollback);
    assert!(err.contains("rollback_unavailable"), "{err}");
    assert!(err.contains("app's Settings"), "{err}");
    assert!(!stdout(&rollback).contains("build="));
    assert_eq!(rig.read_state(), before);
    assert!(rig.release_dir(sha_c).is_dir());
    assert_eq!(rig.link("current"), Some(link_target(sha_a)));

    // the staged launcher, asked directly, names the same reason.
    let qualify = rig
        .command(&rig.release_dir(sha_c).join("ducktape-launcher"))
        .arg("--qualify")
        .arg(rig.state_path())
        .output()
        .unwrap();
    assert!(!qualify.status.success());
    assert_eq!(stdout(&qualify).trim(), "executable_missing");

    // a launcher outside the staged release may not qualify it.
    let elsewhere = rig
        .command(&rig.release_dir(sha_a).join("ducktape-launcher"))
        .arg("--qualify")
        .arg(rig.state_path())
        .output()
        .unwrap();
    assert!(!elsewhere.status.success());
    assert_eq!(stdout(&elsewhere).trim(), "launcher_outside_staged");
}

#[test]
fn a_state_file_that_is_a_symlink_is_refused_and_the_app_beside_the_launcher_runs() {
    let rig = Rig::new();
    let (source_a, _) = rig.build("A");
    assert!(
        rig.launcher(&["install", "--from", source_a.to_str().unwrap()])
            .status
            .success()
    );
    let real = rig.state_path().with_file_name("elsewhere.json");
    fs::rename(rig.state_path(), &real).unwrap();
    std::os::unix::fs::symlink(&real, rig.state_path()).unwrap();
    let booted = rig.installed_launcher(&["duck://x"]);
    assert!(booted.status.success(), "{}", stderr(&booted));
    let out = stdout(&booted);
    assert!(out.contains("build=A"), "{out}");
    assert!(
        out.contains("release=\n"),
        "no release env on the fallback: {out}"
    );
    assert!(out.contains("arg=duck://x"), "{out}");
    let err = stderr(&booted);
    assert!(err.contains("symlink_refused"), "{err}");
    assert!(!err.contains("duck://"), "{err}");
}

#[test]
fn a_release_dir_that_is_a_symlink_is_refused_at_the_flip() {
    let rig = Rig::new();
    let (source_a, sha_a) = rig.build("A");
    assert!(
        rig.launcher(&["install", "--from", source_a.to_str().unwrap()])
            .status
            .success()
    );
    let (source_b, sha_b) = rig.build("B");
    let outside = rig.home.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::copy(source_b.join("ducktape-app"), outside.join("ducktape-app")).unwrap();
    fs::copy(LAUNCHER, outside.join("ducktape-launcher")).unwrap();
    std::os::unix::fs::symlink(&outside, rig.release_dir(sha_b)).unwrap();
    rig.write_state(&staged(sha_a, sha_b));
    let booted = rig.installed_launcher(&[]);
    assert!(booted.status.success(), "{}", stderr(&booted));
    assert!(stdout(&booted).contains("build=A"), "{}", stdout(&booted));
    let err = stderr(&booted);
    assert!(err.contains("symlink_refused"), "{err}");
    assert_eq!(rig.link("current"), Some(link_target(sha_a)));
}

#[test]
fn an_unwritable_install_root_is_refused_by_name() {
    // root ignores directory modes; the check is meaningless there.
    // SAFETY: geteuid takes no arguments and cannot fail.
    let is_root = unsafe { libc::geteuid() } == 0;
    if is_root {
        return;
    }
    let rig = Rig::new();
    let (source_a, _) = rig.build("A");
    fs::create_dir_all(rig.install_dir()).unwrap();
    fs::set_permissions(rig.install_dir(), fs::Permissions::from_mode(0o555)).unwrap();
    let refused = rig.launcher(&["install", "--from", source_a.to_str().unwrap()]);
    fs::set_permissions(rig.install_dir(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!refused.status.success());
    let err = stderr(&refused);
    assert!(err.contains("install_root_not_writable"), "{err}");
    assert!(!rig.state_path().exists());
}

#[test]
fn a_reinstall_of_a_newer_build_keeps_the_old_one_as_previous() {
    let rig = Rig::new();
    let (source_a, sha_a) = rig.build("A");
    assert!(
        rig.launcher(&["install", "--from", source_a.to_str().unwrap()])
            .status
            .success()
    );
    let (source_b, sha_b) = rig.build("B");
    let again = rig.launcher(&["install", "--from", source_b.to_str().unwrap()]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(rig.link("current"), Some(link_target(sha_b)));
    assert_eq!(rig.link("previous"), Some(link_target(sha_a)));
    assert_eq!(
        rig.read_state(),
        Phase::Idle(Idle {
            current: sha_b,
            previous: Some(sha_a),
            pinned_sequence: 0,
        })
    );
    // and the same build again changes nothing.
    let same = rig.launcher(&["install", "--from", source_b.to_str().unwrap()]);
    assert!(same.status.success(), "{}", stderr(&same));
    assert_eq!(rig.link("previous"), Some(link_target(sha_a)));
}

/// The release as packaging stages it: the Linux archive holds
/// `{ducktape-launcher, ducktape-app}` at its root, which is exactly
/// the directory `install --from` takes. It installs under the launcher IT
/// ships (byte for byte, never the one running `install`), and that launcher
/// boots the app. Packing the archive is core's `ops/release/archive.sh` and
/// is tested where the script lives.
#[test]
fn a_release_installs_under_the_launcher_it_ships() {
    let rig = Rig::new();
    let (source, sha) = rig.build("A");
    // the shipped launcher: distinguishable from the one running `install`
    // (a wrapper that execs it), so "kept as shipped" is a byte comparison.
    let shipped = format!("#!/bin/sh\nexec {LAUNCHER} \"$@\"\n");
    fs::write(source.join("ducktape-launcher"), &shipped).unwrap();
    fs::set_permissions(
        source.join("ducktape-launcher"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    // `install` run by a launcher that is NOT the shipped one copies itself
    // in only when the source ships none; this source ships one, so the
    // installed launcher is the shipped file, byte for byte.
    let installed = rig.launcher(&["install", "--from", source.to_str().unwrap()]);
    assert!(installed.status.success(), "{}", stderr(&installed));
    assert_eq!(
        fs::read_to_string(rig.release_dir(sha).join("ducktape-launcher")).unwrap(),
        shipped
    );
    assert_eq!(rig.link("current"), Some(link_target(sha)));
    let booted = rig.installed_launcher(&["duck://forge/ducktape/1?net=abc"]);
    assert!(booted.status.success(), "{}", stderr(&booted));
    assert!(stdout(&booted).contains("build=A"), "{}", stdout(&booted));
    assert!(
        stdout(&booted).contains(&format!("release={sha}")),
        "{}",
        stdout(&booted)
    );
}

/// A flip moves the launcher too: the desktop entry runs
/// `current/ducktape-launcher`, so the release a flip brings in is applied
/// next time by its OWN launcher, never one the flip left behind (#80). B's
/// launcher is the real binary plus a trailing line (the ELF loader ignores
/// it and nothing hashes the launcher), so "moved" is a digest.
#[test]
fn a_flip_moves_the_launcher_that_applies_the_next_release() {
    let rig = Rig::new();
    let (source_a, sha_a) = rig.build("A");
    assert!(
        rig.launcher(&["install", "--from", source_a.to_str().unwrap()])
            .status
            .success()
    );
    let digest = |path: &Path| Sha::digest(&fs::read(path).unwrap());
    let launcher_a = rig.release_dir(sha_a).join("ducktape-launcher");
    let digest_a = digest(&launcher_a);

    let sha_b = rig.stage("B", true);
    let launcher_b = rig.release_dir(sha_b).join("ducktape-launcher");
    fs::OpenOptions::new()
        .append(true)
        .open(&launcher_b)
        .unwrap()
        .write_all(b"\nrelease B\n")
        .unwrap();
    let digest_b = digest(&launcher_b);
    assert_ne!(digest_a, digest_b);

    rig.write_state(&staged(sha_a, sha_b));
    let flipped = rig.installed_launcher(&[]);
    assert!(flipped.status.success(), "{}", stderr(&flipped));
    assert!(stdout(&flipped).contains("build=B"), "{}", stdout(&flipped));
    assert_eq!(rig.link("current"), Some(link_target(sha_b)));
    let current = rig.install_dir().join("current").join("ducktape-launcher");
    assert_eq!(
        current.canonicalize().unwrap(),
        launcher_b.canonicalize().unwrap()
    );
    assert_eq!(digest(&current), digest_b);

    // the next boot goes through B's launcher and execs B's app in its PID.
    let boot = rig
        .command(&current)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = boot.id();
    let booted = boot.wait_with_output().unwrap();
    assert!(booted.status.success(), "{}", stderr(&booted));
    let out = stdout(&booted);
    assert!(out.contains("build=B"), "{out}");
    assert!(out.contains(&format!("release={sha_b}")), "{out}");
    assert!(out.contains(&format!("pid={pid}\n")), "{out}");

    // A's launcher stays where A shipped it, byte for byte.
    assert_eq!(digest(&launcher_a), digest_a);
}

/// `install --release-key` pins the key the app's release channel verifies
/// under, at `<updates>/keys/release.pub` where the app reads it. A malformed
/// key, or one that differs from the key already pinned, is refused by name
/// before the install touches anything; the same key again installs.
#[test]
fn install_pins_the_release_key_and_never_replaces_a_different_one() {
    let rig = Rig::new();
    let (source_a, sha_a) = rig.build("A");
    let key = "ab".repeat(32);
    let installed = rig.launcher(&[
        "install",
        "--from",
        source_a.to_str().unwrap(),
        "--release-key",
        &key,
    ]);
    assert!(installed.status.success(), "{}", stderr(&installed));
    assert_eq!(
        fs::read_to_string(rig.release_key_path()).unwrap(),
        format!("{key}\n")
    );

    let (source_b, sha_b) = rig.build("B");
    let source_b = source_b.to_str().unwrap();
    let malformed = rig.launcher(&["install", "--from", source_b, "--release-key", "abc"]);
    assert!(!malformed.status.success());
    let err = stderr(&malformed);
    assert!(err.contains("release_key_invalid"), "{err}");

    let other = "cd".repeat(32);
    let different = rig.launcher(&["install", "--from", source_b, "--release-key", &other]);
    assert!(!different.status.success());
    let err = stderr(&different);
    assert!(err.contains("release_key_pinned"), "{err}");

    // neither refusal installed B or touched the pin
    assert_eq!(rig.link("current"), Some(link_target(sha_a)));
    assert!(!rig.release_dir(sha_b).exists());
    assert_eq!(
        fs::read_to_string(rig.release_key_path()).unwrap(),
        format!("{key}\n")
    );

    let same = rig.launcher(&["install", "--release-key", &key, "--from", source_b]);
    assert!(same.status.success(), "{}", stderr(&same));
    assert_eq!(rig.link("current"), Some(link_target(sha_b)));
    assert_eq!(
        fs::read_to_string(rig.release_key_path()).unwrap(),
        format!("{key}\n")
    );
}
