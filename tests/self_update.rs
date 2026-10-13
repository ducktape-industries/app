//! The real binary's boot step, on a Linux install under a scratch home:
//! `ducktape-app install` lays the tree; a start that finds a staged release
//! qualifies it, flips, and `exec`s it in the same process with argv
//! untouched (a `duck://` link rides there); a start that finds the release
//! before never rendered flips back and `exec`s the one before it, the same
//! way. The release on the other side is a script that writes down the
//! process it ran in and the arguments it got.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use app_update::{PendingHealthy, Phase, Sha, Staged, state};

const APP: &str = env!("CARGO_BIN_EXE_ducktape-app");

/// A scratch home: the app's config, data, state and cache under one dir.
struct Rig {
    root: tempfile::TempDir,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            root: tempfile::TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap(),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    fn install_dir(&self) -> PathBuf {
        self.path("data/ducktape")
    }

    fn state_path(&self) -> PathBuf {
        self.path("cfg/ducktape/updates/state.json")
    }

    fn release_dir(&self, sha: Sha) -> PathBuf {
        self.install_dir().join("releases").join(sha.to_string())
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env("HOME", self.root.path())
            .env("XDG_CONFIG_HOME", self.path("cfg"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            // should the boot step not exec, the app must not find a screen
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env("RECORD", self.path("record"))
            .stdin(Stdio::null());
        command
    }

    /// `ducktape-app install` of the real binary: its sha and pinned phase.
    fn install(&self) -> (Sha, Phase) {
        let output = self
            .command(Path::new(APP))
            .arg("install")
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{said} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let phase = self.state();
        assert!(
            said.starts_with(&format!("installed {} at ", phase.current())),
            "{said}"
        );
        (phase.current(), phase)
    }

    fn state(&self) -> Phase {
        state::decode(&std::fs::read_to_string(self.state_path()).unwrap()).unwrap()
    }

    fn persist(&self, phase: &Phase) {
        std::fs::write(self.state_path(), state::encode(phase)).unwrap();
    }

    /// A release whose app writes down `<pid> <args>` and exits.
    fn recorder(&self, name: &str) -> Sha {
        use std::os::unix::fs::PermissionsExt as _;
        let sha = Sha::digest(name.as_bytes());
        let dir = self.release_dir(sha);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("ducktape-app");
        std::fs::write(
            &exe,
            "#!/bin/sh\ncase \"$1\" in --version) echo \"ducktape-app 0.0.0+rec\"; exit 0;; esac\necho \"$$ $*\" > \"$RECORD\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        sha
    }

    fn link(&self, name: &str, sha: Sha) {
        let link = self.install_dir().join(name);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(format!("releases/{sha}"), link).unwrap();
    }

    fn points(&self, name: &str) -> PathBuf {
        std::fs::read_link(self.install_dir().join(name)).unwrap()
    }

    /// Start the install path's app as the session does, with `args`: the
    /// pid it started as, and what the recorder wrote down.
    fn start(&self, args: &[&str]) -> (String, String) {
        let pid_file = self.path("pid");
        let mut child = self
            .command(Path::new("/bin/sh"))
            .args(["-c", "echo $$ > \"$PID_FILE\"; exec \"$0\" \"$@\""])
            .arg(self.install_dir().join("current/ducktape-app"))
            .args(args)
            .env("PID_FILE", &pid_file)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("the start did not end");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(status.success(), "{status}");
        let pid = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .to_string();
        let record = std::fs::read_to_string(self.path("record")).unwrap();
        (pid, record.trim().to_string())
    }
}

#[test]
fn a_start_with_a_staged_release_execs_it_in_the_same_process_with_its_link() {
    let rig = Rig::new();
    let (installed, phase) = rig.install();
    let pin = phase.pinned_sequence();
    let staged = rig.recorder("next");
    rig.persist(&Phase::Staged(Staged {
        current: installed,
        previous: None,
        pinned_sequence: pin,
        staged,
        sequence: pin + 1,
        display: "0.0.0+next".into(),
        refused: None,
    }));
    let (pid, record) = rig.start(&["duck://garage/chat/dm 1", "--flag"]);
    assert_eq!(record, format!("{pid} duck://garage/chat/dm 1 --flag"));
    assert_eq!(
        rig.points("current"),
        Path::new("releases").join(staged.to_string())
    );
    assert_eq!(
        rig.points("previous"),
        Path::new("releases").join(installed.to_string())
    );
    assert_eq!(
        rig.state(),
        Phase::PendingHealthy(PendingHealthy {
            current: staged,
            previous: installed,
            boots: 0,
            pinned_sequence: pin + 1,
        })
    );
}

#[test]
fn a_start_after_a_release_that_never_rendered_execs_the_one_before() {
    let rig = Rig::new();
    let (installed, phase) = rig.install();
    let pin = phase.pinned_sequence();
    let before = rig.recorder("before");
    rig.link("previous", before);
    // the installed binary is the release flipped to, and its own boot
    // already counted it once: it never rendered
    rig.persist(&Phase::PendingHealthy(PendingHealthy {
        current: installed,
        previous: before,
        boots: 1,
        pinned_sequence: pin,
    }));
    let (pid, record) = rig.start(&["duck://garage/forge"]);
    assert_eq!(record, format!("{pid} duck://garage/forge"));
    assert_eq!(
        rig.points("current"),
        Path::new("releases").join(before.to_string())
    );
    let Phase::RolledBack(rolled_back) = rig.state() else {
        panic!("{:?}", rig.state());
    };
    assert_eq!(
        (rolled_back.current, rolled_back.failed),
        (before, installed)
    );
}
