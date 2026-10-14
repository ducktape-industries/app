//! The update state machine's vocabulary: `Phase` (persisted verbatim as
//! `state.json`), `Event` (every input, one enum) and `Command` (every
//! effect, executed in order by the executor).
//!
//! One binary drives the machine in two roles that never overlap: the boot
//! step at the top of `main` (flip, rollback, `exec`), then the running app
//! (check, download, stage, the healthy signal). `state.json` is their whole
//! contract. Paths are the executor's: every command names a release by its
//! `Sha` and the executor maps that to `releases/<sha>/` (or `<sha>.partial`).

use serde::{Deserialize, Serialize};

use crate::manifest::SuccessorKey;
use crate::sha::Sha;
use crate::verify::{Refusal, VerifiedManifest};

/// Where the install is. Every transition is persisted tmp-write + rename by
/// the executor's `Persist`; the machine returns the phase it ends in.
///
/// Each variant wraps a named struct so a `step` handler receives the whole
/// variant by value; on the wire it is one flat object tagged `"phase"`.
/// `pinned_sequence` rides in every variant: it is the downgrade guard, it
/// advances only at the flip to a staged release, and a rollback never
/// lowers it — so an equal sequence is a release this install ran.
// `Downloading` carries the manifest's strings; a `Phase` is built once per
// transition and persisted, never held in bulk, so the size gap is nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// Running `current`; `previous` is the one release kept for rollback.
    Idle(Idle),
    /// A newer manifest was accepted; `target` is being downloaded (and then
    /// verified — the machine stays here until `Verified`/`VerifyRefused`).
    Downloading(Downloading),
    /// `staged` is verified, extracted and ready to flip.
    /// The channel stays open: a newer release supersedes it.
    Staged(Staged),
    /// The crash-safe swap bit: persisted before `Flip`, replaced after.
    /// A boot that finds it asks the executor which side landed.
    Swapping(Swapping),
    /// Flipped to `current`; the app has not rendered a frame yet.
    PendingHealthy(PendingHealthy),
    /// The boot step flipped back to `current` because `failed` never
    /// rendered. Read once by the app, then `Idle`.
    RolledBack(RolledBack),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Idle {
    pub current: Sha,
    pub previous: Option<Sha>,
    pub pinned_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Downloading {
    pub current: Sha,
    pub previous: Option<Sha>,
    pub pinned_sequence: u64,
    pub target: Sha,
    pub size: u64,
    pub sequence: u64,
    pub display: String,
    pub successor_key: Option<SuccessorKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub current: Sha,
    pub previous: Option<Sha>,
    pub pinned_sequence: u64,
    pub staged: Sha,
    pub sequence: u64,
    pub display: String,
    /// Why the staged release's own qualify last refused it, if it did. It is
    /// persisted because the boot step qualifies and the app reports it.
    pub refused: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Swapping {
    pub from: Sha,
    pub to: Sha,
    pub pinned_sequence: u64,
}

/// `boots` counts boot steps since the flip. The image the flip `exec`s
/// runs its own boot step first (`boots: 1`), so the next boot without
/// `Rendered` in between means the flipped release never came up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingHealthy {
    pub current: Sha,
    pub previous: Sha,
    pub boots: u8,
    pub pinned_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolledBack {
    pub current: Sha,
    pub failed: Sha,
    pub reason: RollbackReason,
    pub pinned_sequence: u64,
}

impl Phase {
    /// The release that should be running (or exec'd) in this phase. While
    /// `Swapping` it is `to`: the swap is being finished, never reverted
    /// blind.
    pub fn current(&self) -> Sha {
        match self {
            Phase::Idle(Idle { current, .. })
            | Phase::Downloading(Downloading { current, .. })
            | Phase::Staged(Staged { current, .. })
            | Phase::PendingHealthy(PendingHealthy { current, .. })
            | Phase::RolledBack(RolledBack { current, .. }) => *current,
            Phase::Swapping(Swapping { to, .. }) => *to,
        }
    }

    /// The phase's name without its body — what a log line says a machine
    /// settled on.
    pub fn name(&self) -> &'static str {
        match self {
            Phase::Idle(_) => "idle",
            Phase::Downloading(_) => "downloading",
            Phase::Staged(_) => "staged",
            Phase::Swapping(_) => "swapping",
            Phase::PendingHealthy(_) => "pending_healthy",
            Phase::RolledBack(_) => "rolled_back",
        }
    }

    pub fn pinned_sequence(&self) -> u64 {
        match self {
            Phase::Idle(Idle {
                pinned_sequence, ..
            })
            | Phase::Downloading(Downloading {
                pinned_sequence, ..
            })
            | Phase::Staged(Staged {
                pinned_sequence, ..
            })
            | Phase::Swapping(Swapping {
                pinned_sequence, ..
            })
            | Phase::PendingHealthy(PendingHealthy {
                pinned_sequence, ..
            })
            | Phase::RolledBack(RolledBack {
                pinned_sequence, ..
            }) => *pinned_sequence,
        }
    }
}

/// Why the boot step rolled back on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackReason {
    /// The flipped release booted but never rendered a frame (crash, dyld
    /// failure, abort) and the next boot found `PendingHealthy` again.
    NeverRendered,
}

impl std::fmt::Display for RollbackReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RollbackReason::NeverRendered => f.write_str("never_rendered"),
        }
    }
}

/// Every input, from either role.
// `ManifestFetched` carries a whole manifest; one event exists at a time.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Boot step: process start.
    Boot,
    /// Boot step, answering `ResolveSwap`: which side of the swap is live.
    SwapResolved(SwapState),
    /// App: the channel manifest was fetched and its signature checked.
    ManifestFetched(Result<VerifiedManifest, Refusal>),
    /// App: `releases/<sha>.partial` is complete.
    DownloadFinished { sha: Sha },
    /// App: the download could not complete; `reason` is a stable token.
    DownloadFailed { sha: Sha, reason: String },
    /// App: the archive's sha256 matched, it extracted into `releases/<sha>/`
    /// and (macOS) its bundle signature checked.
    Verified(Sha),
    /// App: any of those checks failed; `reason` is a stable token.
    VerifyRefused { sha: Sha, reason: String },
    /// Boot step: the staged release answered its own self-check.
    QualifyPassed(Sha),
    /// It did not; `reason` is a stable token.
    QualifyFailed { sha: Sha, reason: String },
    /// App: the `RolledBack` notice was read.
    DismissRollbackNotice,
    /// The release that was flipped to came up: its first frame. The
    /// healthy signal, and the only thing that clears `PendingHealthy`.
    Rendered,
    /// App: the check cadence; the only time-driven input.
    Tick,
    /// App: the person picked the other channel. A release the old channel
    /// offered is dropped; what runs, and its pin, stay.
    ChannelChanged,
}

/// What the executor found at the install path while `Swapping`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapState {
    /// The install path already resolves to `to`: the flip landed before
    /// the crash.
    Landed,
    /// It still resolves to `from`: the flip never happened.
    Untouched,
}

/// Every effect, in the order the executor performs them. `Persist` carries
/// the phase to write; the phase `step` returns equals the last `Persist` of
/// its command list, so an executor that writes every `Persist` and a test
/// that reads the return value agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Write `state.json` (tmp-write + rename).
    Persist(Phase),
    /// Read the channel's file, manifest and signature ([`crate::layout`]),
    /// verify, answer with `ManifestFetched`.
    Fetch,
    /// Download the archive ([`crate::layout::archive_name`] of `sha` for
    /// the host platform) into `releases/<sha>.partial`, resumable by size,
    /// sha256-checked as it lands; answer with
    /// `DownloadFinished`/`DownloadFailed`.
    Download { sha: Sha, size: u64 },
    /// sha256 the archive against `sha`, extract into `releases/<sha>/`,
    /// macOS `codesign --verify`; answer with `Verified`/`VerifyRefused`.
    Verify(Sha),
    /// Record the announced successor key beside the pinned one.
    PinSuccessor(SuccessorKey),
    /// Ask the staged release whether it runs; answer with
    /// `QualifyPassed`/`QualifyFailed`. Boot step only.
    Qualify(Sha),
    /// Ask which side of a `Swapping` is live; answer with `SwapResolved`.
    /// Boot step only.
    ResolveSwap { from: Sha, to: Sha },
    /// Point the install path at `to` (symlink swap / `RENAME_SWAP`).
    Flip { from: Sha, to: Sha },
    /// Run `sha`: the install path now points at it (a `Flip` earlier in
    /// this list, or one that landed before a crash). A boot with no `Exec`
    /// in its command list runs [`Phase::current`].
    Exec(Sha),
    /// What the app reads out (log, Settings); app only.
    Banner(UpdateBanner),
    /// Remove every `releases/<sha>` not in `keep`, and stale `.partial`s.
    Gc { keep: Vec<Sha> },
}

/// The channel's last word: what the app logs and Settings reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateBanner {
    /// "Ducktape <display> is ready": it takes over at the next start.
    Ready {
        staged: Sha,
        display: String,
    },
    /// The manifest named nothing newer than what runs.
    UpToDate,
    /// The manifest was refused; `reason` is the Settings "last refusal".
    Refused(Refusal),
    DownloadFailed {
        target: Sha,
        reason: String,
    },
    VerifyRefused {
        target: Sha,
        reason: String,
    },
    QualifyFailed {
        staged: Sha,
        reason: String,
    },
}
