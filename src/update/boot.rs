//! The boot step, at the very top of `main` before gpui starts: read
//! `state.json`, run [`step`] on [`Event::Boot`], perform the qualify, flip or
//! rollback it is told, and make sure the image that goes on is the release
//! the install path names, `exec`ing it if not. Same PID, argv passed through
//! untouched (a `duck://` URL rides there), so the process the session
//! started IS the app, and on macOS it stays the one executable the bundle
//! is signed with.
//!
//! The image a flip `exec`s runs this same step first: it finds
//! `PendingHealthy{boots: 0}`, counts itself (`boots: 1`) and goes on. If it
//! never renders a frame, the next start finds `boots: 1` and flips back.
//!
//! When the update machinery cannot be trusted (a link where a file should
//! be, a flip that refused, an `exec` that failed) the process goes on as the
//! app with the updater off and says why under `ducktape::update`: the boot
//! step's one job is to never strand the user.

use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};

use app_update::{Command, Event, Phase, Sha, SwapState, step};
use tracing::{debug, info, warn};

use super::{Layout, Refusal, TARGET};

/// What the boot step decided for this process.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Boot {
    /// Not an installed app: no update state was read or written.
    Off,
    /// This image is the release that should run: the app goes on, its
    /// updater armed with this phase.
    Run(Phase),
    /// The install path names another image: `exec` it.
    Exec(PathBuf),
}

/// Decide, then `exec` or arm the updater. Returns when this process goes
/// on as the app.
pub(crate) fn boot() {
    let (Some(layout), Ok(own)) = (Layout::host(), std::env::current_exe()) else {
        return;
    };
    match boot_at(&layout, &own) {
        Ok(Boot::Off) => {
            debug!(target: TARGET, event = "app_update_off", "not an installed copy: the updater is off");
        }
        Ok(Boot::Run(phase)) => super::channel::arm(layout, phase),
        Ok(Boot::Exec(exe)) => {
            info!(target: TARGET, event = "app_update_exec", exe = %exe.display());
            let error = Process::new(&exe).args(std::env::args_os().skip(1)).exec();
            warn!(target: TARGET, event = "app_update_refused", reason = "exec_failed", %error, "going on as this image with the updater off");
        }
        Err(refusal) => {
            warn!(target: TARGET, event = "app_update_refused", reason = refusal.reason, detail = %refusal.detail, "going on with the updater off");
        }
    }
}

/// The decision over explicit inputs, every file effect performed: `own` is
/// the running executable.
pub(super) fn boot_at(layout: &Layout, own: &Path) -> Result<Boot, Refusal> {
    let has_state = std::fs::symlink_metadata(layout.state_path()).is_ok();
    if !(has_state && layout.owns(own)) {
        return Ok(Boot::Off);
    }
    if !super::hold_install(layout)? {
        debug!(target: TARGET, event = "app_update_held", "another process runs this install's updater");
        return Ok(Boot::Off);
    }
    // taken before anything moves: on macOS the swap puts another bundle
    // at this very path
    let own_id = super::file_id(own)?;
    let phase = super::read_state(&layout.state_path())?.ok_or_else(|| {
        Refusal::new(
            "state_missing",
            format!("{} does not exist", layout.state_path().display()),
        )
    })?;
    let phase = drive(layout, phase)?;
    if cfg!(not(target_os = "macos")) {
        let points_at = super::read_link(&layout.current_link())?;
        let names = Layout::link_target(phase.current());
        if points_at.as_deref() != Some(names.as_path()) {
            return Err(Refusal::new(
                "install_path_mismatch",
                format!(
                    "{} points at {points_at:?}, not {}",
                    layout.current_link().display(),
                    names.display()
                ),
            ));
        }
    }
    let exe = layout.app_exe();
    super::require_executable(&exe)?;
    match super::file_id(&exe)? == own_id {
        true => Ok(Boot::Run(phase)),
        false => Ok(Boot::Exec(exe)),
    }
}

/// Step the machine from `Boot`, performing each command in order and
/// feeding the answers (`SwapResolved`, `QualifyPassed`/`Failed`) back in,
/// until a list asks for nothing more. Returns the phase it settled on.
/// `Exec` needs nothing here: [`boot_at`] runs whatever the install path
/// names once the machine is done with it.
fn drive(layout: &Layout, mut phase: Phase) -> Result<Phase, Refusal> {
    let mut event = Event::Boot;
    loop {
        let (next, commands) = step(phase, event);
        phase = next;
        let mut answer = None;
        for command in commands {
            match command {
                Command::Persist(written) => super::persist_phase(layout, &written)?,
                Command::Flip { from, to } => {
                    flip(layout, from, to)?;
                    info!(target: TARGET, event = "app_update_flipped", %from, %to);
                }
                Command::ResolveSwap { from, to } => {
                    answer = Some(Event::SwapResolved(resolve(layout, from, to)?));
                }
                Command::Qualify(sha) => answer = Some(qualify(layout, sha)),
                Command::Exec(sha) => {
                    debug!(target: TARGET, release = %sha, "the install path names it")
                }
                Command::Banner(banner) => super::channel::note_banner(&banner),
                command @ (Command::Fetch
                | Command::Download { .. }
                | Command::Verify(_)
                | Command::SealImmutable(_)
                | Command::PinSuccessor(_)
                | Command::Gc { .. }) => {
                    return Err(Refusal::new(
                        "app_only_command",
                        format!("a boot was told to {command:?}; that is the running app's"),
                    ));
                }
            }
        }
        match answer {
            Some(answered) => event = answered,
            None => return Ok(phase),
        }
    }
}

/// Point the install path at `to`. Linux: `previous` then `current`, each a
/// symlink at a temp name renamed over, so the install path always resolves
/// to one whole release. macOS: the bundle swap.
fn flip(layout: &Layout, from: Sha, to: Sha) -> Result<(), Refusal> {
    if cfg!(target_os = "macos") {
        return super::macos::swap_bundles(layout, from, to);
    }
    super::require_release(layout, to)?;
    super::replace_symlink(&layout.previous_link(), &Layout::link_target(from))?;
    super::replace_symlink(&layout.current_link(), &Layout::link_target(to))
}

/// Which side of a crashed swap is live.
fn resolve(layout: &Layout, from: Sha, to: Sha) -> Result<SwapState, Refusal> {
    if cfg!(target_os = "macos") {
        return super::macos::resolve_swap(layout, from, to);
    }
    let points_at = super::read_link(&layout.current_link())?;
    if points_at.as_deref() == Some(Layout::link_target(to).as_path()) {
        super::replace_symlink(&layout.previous_link(), &Layout::link_target(from))?;
        return Ok(SwapState::Landed);
    }
    if points_at.as_deref() == Some(Layout::link_target(from).as_path()) {
        return Ok(SwapState::Untouched);
    }
    Err(Refusal::new(
        "install_path_unknown",
        format!(
            "{} points at {points_at:?}, neither side of the swap",
            layout.current_link().display()
        ),
    ))
}

/// The staged release, asked whether it runs at all before anything flips
/// to it: whole, (macOS) its bundle signature intact, and its own
/// `--version` answered — the one flag every release answers without
/// starting the app. A binary that does not load fails here, not at the
/// boot after the flip.
fn qualify(layout: &Layout, sha: Sha) -> Event {
    let whole =
        super::require_release(layout, sha).and_then(|()| match cfg!(target_os = "macos") {
            true => super::macos::verify_bundle(&layout.staged_bundle(sha)),
            false => Ok(()),
        });
    let answered = whole
        .map_err(|refusal| refusal.reason.to_string())
        .and_then(|()| answers_version(&layout.release_exe(sha)));
    match answered {
        Ok(()) => Event::QualifyPassed(sha),
        Err(reason) => {
            warn!(target: TARGET, event = "app_update_refused", %sha, %reason, "the staged release failed to qualify; staying on the current one");
            Event::QualifyFailed { sha, reason }
        }
    }
}

/// `<exe> --version` exits 0 and names itself.
fn answers_version(exe: &Path) -> Result<(), String> {
    let output = Process::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "qualify_spawn_failed".to_string())?;
    match (
        output.status.code(),
        output.stdout.starts_with(b"ducktape-app "),
    ) {
        (Some(0), true) => Ok(()),
        (Some(0), false) => Err("qualify_unrecognized".into()),
        (Some(code), _) => Err(format!("qualify_exit_{code}")),
        (None, _) => Err("qualify_killed".into()),
    }
}
