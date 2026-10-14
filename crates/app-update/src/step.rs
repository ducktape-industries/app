//! `step`: the one visible dispatch. Every `(Phase, Event)` pair is routed
//! by the single `match` in [`step`] to a handler named for the pair; the
//! pairs that mean nothing in a phase are listed there too and leave the
//! phase unchanged with no commands. No `_` arm: a new `Event` or `Phase`
//! variant must fail the build until it is routed (a source-parsing test in
//! this file holds that shape).
//!
//! Handlers DECIDE and return the next phase plus commands; they never
//! perform an effect. The executor performs the commands in order.
//!
//! The flows, as the executor sees them:
//! - `Idle` + `Tick` → `Fetch`; `ManifestFetched(Ok)` newer than the pin
//!   with a host artifact that is not `current` → `Downloading` +
//!   `Download`; equal to the pin → `Banner(UpToDate)`; older →
//!   `Refused(SequenceNotNewer)`. `DownloadFinished` → `Verify`; `Verified` →
//!   (`PinSuccessor`) → `Staged` + `Banner(Ready)`.
//! - `Boot` while `Staged` → `Qualify`; `QualifyPassed` → `Persist(Swapping)`
//!   → `Flip` → `Persist(PendingHealthy{boots: 0})` → `Exec`, the swap
//!   adopting the staged sequence as the pin; `QualifyFailed` → stay
//!   `Staged` with the reason persisted as `refused`, `Banner` names it.
//! - A staged release does not close the channel: `Staged` + `Tick` →
//!   `Fetch`. A newer sequence discards the staged release (`Persist(Idle)`,
//!   `Gc`) and is offered as `Idle` offers it. The staged sequence, or a
//!   lower one, changes nothing and repeats no banner. A discarded release
//!   never ran, so the pin never took its sequence and it is offered again
//!   like any newer one.
//! - `PendingHealthy` + `Rendered` → `Idle` + `Gc`. `Boot` with `boots == 0`
//!   (the exec'd image's own boot step) → `boots: 1`; `Boot` with
//!   `boots ≥ 1` (the next start, and nothing rendered) → flip back →
//!   `RolledBack` → `Exec`. `Boot` while `Swapping` → `ResolveSwap`;
//!   `SwapResolved` finishes the swap from whichever side landed.
//!   `pinned_sequence` never lowers, so the release rolled back from is not
//!   downloaded again: a newer sequence supersedes it.
//! - `ChannelChanged` drops a release the old channel offered: `Downloading`
//!   or `Staged` → `Persist(Idle)` + `Gc`. What runs and its pin stay, so a
//!   switch never downgrades: the next check takes the new channel's release
//!   only when its sequence is newer than the pin. Sequences are one order
//!   across channels (a release's sequence is its commit's committer time).
//! - A boot whose command list carries no `Exec` runs [`Phase::current`].

use std::cmp::Ordering;

use crate::manifest::Platform;
use crate::phase::{
    Command, Downloading, Event, Idle, PendingHealthy, Phase, RollbackReason, RolledBack, Staged,
    SwapState, Swapping, UpdateBanner,
};
use crate::sha::Sha;
use crate::verify::{Refusal, VerifiedManifest};

/// Advance the machine by one event.
pub fn step(phase: Phase, event: Event) -> (Phase, Vec<Command>) {
    match (phase, event) {
        // Idle: the only phase that checks for updates from scratch.
        (Phase::Idle(idle), Event::Tick) => idle_tick(idle),
        (Phase::Idle(idle), Event::ManifestFetched(result)) => idle_manifest_fetched(idle, result),
        (
            phase @ Phase::Idle(_),
            Event::Boot
            | Event::SwapResolved(_)
            | Event::DownloadFinished { .. }
            | Event::DownloadFailed { .. }
            | Event::Verified(_)
            | Event::VerifyRefused { .. }
            | Event::QualifyPassed(_)
            | Event::QualifyFailed { .. }
            | Event::DismissRollbackNotice
            | Event::Rendered
            | Event::ChannelChanged,
        ) => unchanged(phase),

        // Downloading: the app is fetching, then verifying, `target`.
        (Phase::Downloading(downloading), Event::Boot) => downloading_boot(downloading),
        (Phase::Downloading(downloading), Event::DownloadFinished { sha }) => {
            downloading_download_finished(downloading, sha)
        }
        (Phase::Downloading(downloading), Event::DownloadFailed { sha, reason }) => {
            downloading_download_failed(downloading, sha, reason)
        }
        (Phase::Downloading(downloading), Event::Verified(sha)) => {
            downloading_verified(downloading, sha)
        }
        (Phase::Downloading(downloading), Event::VerifyRefused { sha, reason }) => {
            downloading_verify_refused(downloading, sha, reason)
        }
        (Phase::Downloading(downloading), Event::ChannelChanged) => {
            downloading_channel_changed(downloading)
        }
        (
            phase @ Phase::Downloading(_),
            Event::SwapResolved(_)
            | Event::ManifestFetched(_)
            | Event::QualifyPassed(_)
            | Event::QualifyFailed { .. }
            | Event::DismissRollbackNotice
            | Event::Rendered
            | Event::Tick,
        ) => unchanged(phase),

        // Staged: a verified release waits for the next start; a newer one
        // supersedes it.
        (Phase::Staged(staged), Event::Boot) => staged_boot(staged),
        (Phase::Staged(staged), Event::QualifyPassed(sha)) => staged_qualify_passed(staged, sha),
        (Phase::Staged(staged), Event::QualifyFailed { sha, reason }) => {
            staged_qualify_failed(staged, sha, reason)
        }
        (Phase::Staged(staged), Event::Tick) => staged_tick(staged),
        (Phase::Staged(staged), Event::ManifestFetched(result)) => {
            staged_manifest_fetched(staged, result)
        }
        (Phase::Staged(staged), Event::ChannelChanged) => staged_channel_changed(staged),
        (
            phase @ Phase::Staged(_),
            Event::SwapResolved(_)
            | Event::DownloadFinished { .. }
            | Event::DownloadFailed { .. }
            | Event::Verified(_)
            | Event::VerifyRefused { .. }
            | Event::DismissRollbackNotice
            | Event::Rendered,
        ) => unchanged(phase),

        // Swapping: only a boot ever sees it (a crash mid-flip).
        (Phase::Swapping(swapping), Event::Boot) => swapping_boot(swapping),
        (Phase::Swapping(swapping), Event::SwapResolved(state)) => {
            swapping_resolved(swapping, state)
        }
        (
            phase @ Phase::Swapping(_),
            Event::ManifestFetched(_)
            | Event::DownloadFinished { .. }
            | Event::DownloadFailed { .. }
            | Event::Verified(_)
            | Event::VerifyRefused { .. }
            | Event::QualifyPassed(_)
            | Event::QualifyFailed { .. }
            | Event::DismissRollbackNotice
            | Event::Rendered
            | Event::Tick
            | Event::ChannelChanged,
        ) => unchanged(phase),

        // PendingHealthy: flipped, waiting for the first frame.
        (Phase::PendingHealthy(pending), Event::Boot) => pending_healthy_boot(pending),
        (Phase::PendingHealthy(pending), Event::Rendered) => pending_healthy_rendered(pending),
        (
            phase @ Phase::PendingHealthy(_),
            Event::SwapResolved(_)
            | Event::ManifestFetched(_)
            | Event::DownloadFinished { .. }
            | Event::DownloadFailed { .. }
            | Event::Verified(_)
            | Event::VerifyRefused { .. }
            | Event::QualifyPassed(_)
            | Event::QualifyFailed { .. }
            | Event::DismissRollbackNotice
            | Event::Tick
            | Event::ChannelChanged,
        ) => unchanged(phase),

        // RolledBack: the notice stands until the app has read it.
        (Phase::RolledBack(rolled_back), Event::DismissRollbackNotice) => {
            rolled_back_dismiss(rolled_back)
        }
        (
            phase @ Phase::RolledBack(_),
            Event::Boot
            | Event::SwapResolved(_)
            | Event::ManifestFetched(_)
            | Event::DownloadFinished { .. }
            | Event::DownloadFailed { .. }
            | Event::Verified(_)
            | Event::VerifyRefused { .. }
            | Event::QualifyPassed(_)
            | Event::QualifyFailed { .. }
            | Event::Rendered
            | Event::Tick
            | Event::ChannelChanged,
        ) => unchanged(phase),
    }
}

fn unchanged(phase: Phase) -> (Phase, Vec<Command>) {
    (phase, Vec::new())
}

fn banner_only(phase: Phase, banner: UpdateBanner) -> (Phase, Vec<Command>) {
    (phase, vec![Command::Banner(banner)])
}

// --- Idle ------------------------------------------------------------------

fn idle_tick(idle: Idle) -> (Phase, Vec<Command>) {
    (Phase::Idle(idle), vec![Command::Fetch])
}

fn idle_manifest_fetched(
    idle: Idle,
    result: Result<VerifiedManifest, Refusal>,
) -> (Phase, Vec<Command>) {
    match result {
        Ok(verified) => idle_offer(idle, verified),
        Err(refusal) => banner_only(Phase::Idle(idle), UpdateBanner::Refused(refusal)),
    }
}

/// A verified manifest, against the pin. The pin is the sequence of the
/// release this install last flipped to (or was installed from), and it
/// never lowers — so an EQUAL sequence is the release that runs, or the one a
/// rollback left as `previous`, which a newer sequence supersedes. Neither is
/// downloaded again. An older sequence is a downgrade; a newer one is offered.
fn idle_offer(idle: Idle, verified: VerifiedManifest) -> (Phase, Vec<Command>) {
    match verified.manifest.sequence.cmp(&idle.pinned_sequence) {
        Ordering::Less => banner_only(
            Phase::Idle(idle),
            UpdateBanner::Refused(Refusal::SequenceNotNewer),
        ),
        Ordering::Equal => banner_only(Phase::Idle(idle), UpdateBanner::UpToDate),
        Ordering::Greater => offer_newer(idle, verified),
    }
}

/// A newer sequence: take it if it ships this platform and names something
/// other than what runs.
fn offer_newer(idle: Idle, verified: VerifiedManifest) -> (Phase, Vec<Command>) {
    let manifest = verified.manifest;
    let Some(artifact) = manifest.artifact_for(Platform::HOST) else {
        return banner_only(
            Phase::Idle(idle),
            UpdateBanner::Refused(Refusal::NoArtifactForPlatform),
        );
    };
    let already_running = artifact.sha256 == idle.current;
    if already_running {
        return banner_only(Phase::Idle(idle), UpdateBanner::UpToDate);
    }
    let downloading = Downloading {
        current: idle.current,
        previous: idle.previous,
        pinned_sequence: idle.pinned_sequence,
        target: artifact.sha256,
        size: artifact.size,
        sequence: manifest.sequence,
        display: manifest.release.display.clone(),
        successor_key: manifest.successor_key.clone(),
    };
    let download = Command::Download {
        sha: downloading.target,
        size: downloading.size,
    };
    let phase = Phase::Downloading(downloading);
    (phase.clone(), vec![Command::Persist(phase), download])
}

/// The crash-safe flip: `Swapping` persisted BEFORE `Flip`, `PendingHealthy`
/// AFTER, then `Exec`. Shared by the update and the resumed swap.
fn swap_into(from: Sha, to: Sha, pinned_sequence: u64) -> (Phase, Vec<Command>) {
    let swapping = Phase::Swapping(Swapping {
        from,
        to,
        pinned_sequence,
    });
    let (pending, finish) = finish_swap(from, to, pinned_sequence);
    let mut commands = vec![Command::Persist(swapping), Command::Flip { from, to }];
    commands.extend(finish);
    (pending, commands)
}

/// After the flip landed: persist `PendingHealthy{boots: 0}` and exec `to`.
fn finish_swap(from: Sha, to: Sha, pinned_sequence: u64) -> (Phase, Vec<Command>) {
    let pending = Phase::PendingHealthy(PendingHealthy {
        current: to,
        previous: from,
        boots: 0,
        pinned_sequence,
    });
    (
        pending.clone(),
        vec![Command::Persist(pending), Command::Exec(to)],
    )
}

// --- Downloading -----------------------------------------------------------

/// A boot mid-download abandons it: the next `Tick` re-offers and the
/// executor resumes the `.partial` by size.
fn downloading_boot(downloading: Downloading) -> (Phase, Vec<Command>) {
    let idle = Phase::Idle(Idle {
        current: downloading.current,
        previous: downloading.previous,
        pinned_sequence: downloading.pinned_sequence,
    });
    (idle.clone(), vec![Command::Persist(idle)])
}
fn downloading_download_finished(downloading: Downloading, sha: Sha) -> (Phase, Vec<Command>) {
    let is_stale = sha != downloading.target;
    if is_stale {
        return unchanged(Phase::Downloading(downloading));
    }
    (Phase::Downloading(downloading), vec![Command::Verify(sha)])
}

fn downloading_download_failed(
    downloading: Downloading,
    sha: Sha,
    reason: String,
) -> (Phase, Vec<Command>) {
    let is_stale = sha != downloading.target;
    if is_stale {
        return unchanged(Phase::Downloading(downloading));
    }
    let banner = UpdateBanner::DownloadFailed {
        target: sha,
        reason,
    };
    abandon_download(downloading, banner)
}

/// Verified: record any announced successor key, and offer the restart. The pin stays: it advances at the flip.
fn downloading_verified(downloading: Downloading, sha: Sha) -> (Phase, Vec<Command>) {
    let is_stale = sha != downloading.target;
    if is_stale {
        return unchanged(Phase::Downloading(downloading));
    }
    let staged = Phase::Staged(Staged {
        current: downloading.current,
        previous: downloading.previous,
        pinned_sequence: downloading.pinned_sequence,
        staged: downloading.target,
        sequence: downloading.sequence,
        display: downloading.display.clone(),
        refused: None,
    });
    let banner = UpdateBanner::Ready {
        staged: downloading.target,
        display: downloading.display,
    };
    let mut commands: Vec<Command> = downloading
        .successor_key
        .map(Command::PinSuccessor)
        .into_iter()
        .collect();
    commands.push(Command::Persist(staged.clone()));
    commands.push(Command::Banner(banner));
    (staged, commands)
}

fn downloading_verify_refused(
    downloading: Downloading,
    sha: Sha,
    reason: String,
) -> (Phase, Vec<Command>) {
    let is_stale = sha != downloading.target;
    if is_stale {
        return unchanged(Phase::Downloading(downloading));
    }
    let banner = UpdateBanner::VerifyRefused {
        target: sha,
        reason,
    };
    abandon_download(downloading, banner)
}

/// Back to `Idle` without advancing the pin, and say why.
/// The other channel picked: the download was the old channel's offer. Its
/// partial goes with the next `Gc`.
fn downloading_channel_changed(downloading: Downloading) -> (Phase, Vec<Command>) {
    let idle = Idle {
        current: downloading.current,
        previous: downloading.previous,
        pinned_sequence: downloading.pinned_sequence,
    };
    let keep = std::iter::once(idle.current).chain(idle.previous).collect();
    let commands = vec![
        Command::Persist(Phase::Idle(idle.clone())),
        Command::Gc { keep },
    ];
    (Phase::Idle(idle), commands)
}

fn abandon_download(downloading: Downloading, banner: UpdateBanner) -> (Phase, Vec<Command>) {
    let idle = Phase::Idle(Idle {
        current: downloading.current,
        previous: downloading.previous,
        pinned_sequence: downloading.pinned_sequence,
    });
    (
        idle.clone(),
        vec![Command::Persist(idle), Command::Banner(banner)],
    )
}

// --- Staged ----------------------------------------------------------------

fn staged_boot(staged: Staged) -> (Phase, Vec<Command>) {
    let qualify = Command::Qualify(staged.staged);
    (Phase::Staged(staged), vec![qualify])
}

/// The flip adopts the staged sequence as the pin.
fn staged_qualify_passed(staged: Staged, sha: Sha) -> (Phase, Vec<Command>) {
    let is_stale = sha != staged.staged;
    if is_stale {
        return unchanged(Phase::Staged(staged));
    }
    let adopted = staged.pinned_sequence.max(staged.sequence);
    swap_into(staged.current, staged.staged, adopted)
}

/// Refused by its own self-check: keep running `current` and keep the release
/// staged; the next start asks it again. The reason is persisted: the boot
/// step qualifies, and the running app reports it. The refusal does not close
/// the channel.
fn staged_qualify_failed(staged: Staged, sha: Sha, reason: String) -> (Phase, Vec<Command>) {
    let is_stale = sha != staged.staged;
    if is_stale {
        return unchanged(Phase::Staged(staged));
    }
    let refused = Phase::Staged(Staged {
        refused: Some(reason.clone()),
        ..staged
    });
    let banner = UpdateBanner::QualifyFailed {
        staged: sha,
        reason,
    };
    (
        refused.clone(),
        vec![Command::Persist(refused), Command::Banner(banner)],
    )
}

fn staged_tick(staged: Staged) -> (Phase, Vec<Command>) {
    (Phase::Staged(staged), vec![Command::Fetch])
}

fn staged_manifest_fetched(
    staged: Staged,
    result: Result<VerifiedManifest, Refusal>,
) -> (Phase, Vec<Command>) {
    match result {
        Ok(verified) => staged_offer(staged, verified),
        Err(refusal) => banner_only(Phase::Staged(staged), UpdateBanner::Refused(refusal)),
    }
}

/// A newer sequence supersedes the staged release: discard it, then offer the
/// manifest as `Idle` would. The staged sequence, or a lower one, changes
/// nothing and says nothing: a check repeats every hour for as long as the
/// release waits, `Ready` was said once when it staged, and a refusal is read
/// from `Staged::refused`, not re-announced.
fn staged_offer(staged: Staged, verified: VerifiedManifest) -> (Phase, Vec<Command>) {
    let supersedes = verified.manifest.sequence > staged.sequence;
    if !supersedes {
        return unchanged(Phase::Staged(staged));
    }
    let (idle, mut commands) = discard_staged(staged);
    let (phase, offer) = idle_offer(idle, verified);
    commands.extend(offer);
    (phase, commands)
}

/// The other channel picked: the staged release was the old channel's offer.
fn staged_channel_changed(staged: Staged) -> (Phase, Vec<Command>) {
    let (idle, commands) = discard_staged(staged);
    (Phase::Idle(idle), commands)
}

/// Back to `Idle` on what runs, the pin kept, the staged directory collected.
/// `Persist` comes before `Gc`, so a crash between them leaves `Idle` and an
/// orphan directory, never a `Staged` phase with no release on disk.
fn discard_staged(staged: Staged) -> (Idle, Vec<Command>) {
    let idle = Idle {
        current: staged.current,
        previous: staged.previous,
        pinned_sequence: staged.pinned_sequence,
    };
    let keep = std::iter::once(idle.current).chain(idle.previous).collect();
    let commands = vec![
        Command::Persist(Phase::Idle(idle.clone())),
        Command::Gc { keep },
    ];
    (idle, commands)
}

// --- Swapping --------------------------------------------------------------

/// Crashed mid-flip: never guess, ask which side is live.
fn swapping_boot(swapping: Swapping) -> (Phase, Vec<Command>) {
    let resolve = Command::ResolveSwap {
        from: swapping.from,
        to: swapping.to,
    };
    (Phase::Swapping(swapping), vec![resolve])
}

fn swapping_resolved(swapping: Swapping, state: SwapState) -> (Phase, Vec<Command>) {
    match state {
        SwapState::Landed => finish_swap(swapping.from, swapping.to, swapping.pinned_sequence),
        SwapState::Untouched => {
            let (pending, finish) =
                finish_swap(swapping.from, swapping.to, swapping.pinned_sequence);
            let flip = Command::Flip {
                from: swapping.from,
                to: swapping.to,
            };
            let mut commands = vec![flip];
            commands.extend(finish);
            (pending, commands)
        }
    }
}

// --- PendingHealthy --------------------------------------------------------

/// The exec'd image's own boot counts the flip; a boot after that, with no
/// `Rendered` in between, means the release never came up: flip back,
/// crash-safely, and exec the previous release.
fn pending_healthy_boot(pending: PendingHealthy) -> (Phase, Vec<Command>) {
    let first_boot_since_flip = pending.boots == 0;
    if first_boot_since_flip {
        let counted = Phase::PendingHealthy(PendingHealthy {
            boots: 1,
            ..pending
        });
        return (counted.clone(), vec![Command::Persist(counted)]);
    }
    let swapping = Phase::Swapping(Swapping {
        from: pending.current,
        to: pending.previous,
        pinned_sequence: pending.pinned_sequence,
    });
    let rolled_back = Phase::RolledBack(RolledBack {
        current: pending.previous,
        failed: pending.current,
        reason: RollbackReason::NeverRendered,
        pinned_sequence: pending.pinned_sequence,
    });
    let commands = vec![
        Command::Persist(swapping),
        Command::Flip {
            from: pending.current,
            to: pending.previous,
        },
        Command::Persist(rolled_back.clone()),
        Command::Exec(pending.previous),
    ];
    (rolled_back, commands)
}

/// The healthy signal: keep `current` and `previous`, drop the rest.
fn pending_healthy_rendered(pending: PendingHealthy) -> (Phase, Vec<Command>) {
    let idle = Phase::Idle(Idle {
        current: pending.current,
        previous: Some(pending.previous),
        pinned_sequence: pending.pinned_sequence,
    });
    let gc = Command::Gc {
        keep: vec![pending.current, pending.previous],
    };
    (idle.clone(), vec![Command::Persist(idle), gc])
}

// --- RolledBack ------------------------------------------------------------

/// Read: `failed` stays as `previous`, and the pin keeps its sequence, so it
/// is not downloaded again; a newer sequence supersedes it.
fn rolled_back_dismiss(rolled_back: RolledBack) -> (Phase, Vec<Command>) {
    let idle = Phase::Idle(Idle {
        current: rolled_back.current,
        previous: Some(rolled_back.failed),
        pinned_sequence: rolled_back.pinned_sequence,
    });
    (idle.clone(), vec![Command::Persist(idle)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::testkit::sample;

    fn sha(name: &str) -> Sha {
        Sha::digest(name.as_bytes())
    }

    fn idle(current: &str, previous: Option<&str>, pinned_sequence: u64) -> Phase {
        Phase::Idle(Idle {
            current: sha(current),
            previous: previous.map(sha),
            pinned_sequence,
        })
    }

    fn downloading(target: &str, sequence: u64) -> Downloading {
        Downloading {
            current: sha("a"),
            previous: Some(sha("z")),
            pinned_sequence: 17,
            target: sha(target),
            size: 42,
            sequence,
            display: "0.1.0+b".into(),
            successor_key: None,
        }
    }

    fn staged(current: &str, staged: &str) -> Staged {
        Staged {
            current: sha(current),
            previous: Some(sha("z")),
            pinned_sequence: 17,
            staged: sha(staged),
            sequence: 18,
            display: "0.1.0+b".into(),
            refused: None,
        }
    }

    fn refused(current: &str, staged_sha: &str, reason: &str) -> Phase {
        Phase::Staged(Staged {
            refused: Some(reason.into()),
            ..staged(current, staged_sha)
        })
    }

    fn swapping(from: &str, to: &str) -> Swapping {
        Swapping {
            from: sha(from),
            to: sha(to),
            pinned_sequence: 18,
        }
    }

    fn pending(current: &str, previous: &str, boots: u8) -> Phase {
        Phase::PendingHealthy(PendingHealthy {
            current: sha(current),
            previous: sha(previous),
            boots,
            pinned_sequence: 18,
        })
    }

    fn rolled_back(current: &str, failed: &str) -> Phase {
        Phase::RolledBack(RolledBack {
            current: sha(current),
            failed: sha(failed),
            reason: RollbackReason::NeverRendered,
            pinned_sequence: 18,
        })
    }

    fn fetched(sequence: u64, platform_key: &str, artifact: &str) -> Event {
        let manifest = sample(sequence, platform_key, sha(artifact));
        Event::ManifestFetched(Ok(VerifiedManifest { manifest }))
    }

    /// The whole flip, from `from` to `to`: swap bit, flip, pending, exec.
    fn flip_commands(from: &str, to: &str, pinned_sequence: u64) -> (Phase, Vec<Command>) {
        let pending = Phase::PendingHealthy(PendingHealthy {
            current: sha(to),
            previous: sha(from),
            boots: 0,
            pinned_sequence,
        });
        let commands = vec![
            Command::Persist(Phase::Swapping(Swapping {
                from: sha(from),
                to: sha(to),
                pinned_sequence,
            })),
            Command::Flip {
                from: sha(from),
                to: sha(to),
            },
            Command::Persist(pending.clone()),
            Command::Exec(sha(to)),
        ];
        (pending, commands)
    }

    struct Case {
        name: &'static str,
        phase: Phase,
        event: Event,
        expect: (Phase, Vec<Command>),
    }

    fn host() -> String {
        Platform::HOST.key()
    }

    fn table() -> Vec<Case> {
        let staged_ready = Phase::Staged(Staged {
            current: sha("a"),
            previous: Some(sha("z")),
            pinned_sequence: 17,
            staged: sha("b"),
            sequence: 18,
            display: "0.1.0+b".into(),
            refused: None,
        });
        let accepted = Downloading {
            current: sha("a"),
            previous: Some(sha("z")),
            pinned_sequence: 17,
            target: sha("b"),
            size: 42,
            sequence: 18,
            display: "0.1.0+9d71b254".into(),
            successor_key: None,
        };
        vec![
            // ---- Idle
            Case {
                name: "idle tick fetches",
                phase: idle("a", None, 17),
                event: Event::Tick,
                expect: (idle("a", None, 17), vec![Command::Fetch]),
            },
            Case {
                name: "idle newer manifest downloads",
                phase: idle("a", Some("z"), 17),
                event: fetched(18, &host(), "b"),
                expect: (
                    Phase::Downloading(accepted.clone()),
                    vec![
                        Command::Persist(Phase::Downloading(accepted.clone())),
                        Command::Download {
                            sha: sha("b"),
                            size: 42,
                        },
                    ],
                ),
            },
            Case {
                name: "idle equal sequence is the release installed from its archive's directory",
                phase: idle("binary", None, 18),
                event: fetched(18, &host(), "b"),
                expect: (
                    idle("binary", None, 18),
                    vec![Command::Banner(UpdateBanner::UpToDate)],
                ),
            },
            Case {
                name: "idle equal sequence after a rollback is not downloaded again",
                phase: idle("a", Some("b"), 18),
                event: fetched(18, &host(), "b"),
                expect: (
                    idle("a", Some("b"), 18),
                    vec![Command::Banner(UpdateBanner::UpToDate)],
                ),
            },
            Case {
                name: "idle lower sequence is a downgrade",
                phase: idle("a", None, 17),
                event: fetched(16, &host(), "b"),
                expect: (
                    idle("a", None, 17),
                    vec![Command::Banner(UpdateBanner::Refused(
                        Refusal::SequenceNotNewer,
                    ))],
                ),
            },
            Case {
                name: "idle manifest without this platform",
                phase: idle("a", None, 17),
                event: fetched(18, "plan9-mips", "b"),
                expect: (
                    idle("a", None, 17),
                    vec![Command::Banner(UpdateBanner::Refused(
                        Refusal::NoArtifactForPlatform,
                    ))],
                ),
            },
            Case {
                name: "idle manifest naming what runs is up to date",
                phase: idle("a", None, 17),
                event: fetched(18, &host(), "a"),
                expect: (
                    idle("a", None, 17),
                    vec![Command::Banner(UpdateBanner::UpToDate)],
                ),
            },
            Case {
                name: "idle refused manifest is a banner",
                phase: idle("a", None, 17),
                event: Event::ManifestFetched(Err(Refusal::BadSignature)),
                expect: (
                    idle("a", None, 17),
                    vec![Command::Banner(UpdateBanner::Refused(
                        Refusal::BadSignature,
                    ))],
                ),
            },
            Case {
                name: "idle boot runs current by the executor rule",
                phase: idle("a", None, 17),
                event: Event::Boot,
                expect: (idle("a", None, 17), vec![]),
            },
            Case {
                name: "idle rendered is nothing",
                phase: idle("a", None, 17),
                event: Event::Rendered,
                expect: (idle("a", None, 17), vec![]),
            },
            // ---- Downloading
            Case {
                name: "downloading finished verifies",
                phase: Phase::Downloading(downloading("b", 18)),
                event: Event::DownloadFinished { sha: sha("b") },
                expect: (
                    Phase::Downloading(downloading("b", 18)),
                    vec![Command::Verify(sha("b"))],
                ),
            },
            Case {
                name: "downloading finished for another sha is stale",
                phase: Phase::Downloading(downloading("b", 18)),
                event: Event::DownloadFinished { sha: sha("q") },
                expect: (Phase::Downloading(downloading("b", 18)), vec![]),
            },
            Case {
                name: "downloading failed returns to idle without advancing the pin",
                phase: Phase::Downloading(downloading("b", 18)),
                event: Event::DownloadFailed {
                    sha: sha("b"),
                    reason: "http_503".into(),
                },
                expect: (
                    idle("a", Some("z"), 17),
                    vec![
                        Command::Persist(idle("a", Some("z"), 17)),
                        Command::Banner(UpdateBanner::DownloadFailed {
                            target: sha("b"),
                            reason: "http_503".into(),
                        }),
                    ],
                ),
            },
            Case {
                name: "downloading verified stages, the pin kept for the flip",
                phase: Phase::Downloading(downloading("b", 18)),
                event: Event::Verified(sha("b")),
                expect: (
                    staged_ready.clone(),
                    vec![
                        Command::Persist(staged_ready.clone()),
                        Command::Banner(UpdateBanner::Ready {
                            staged: sha("b"),
                            display: "0.1.0+b".into(),
                        }),
                    ],
                ),
            },
            Case {
                name: "downloading verify refused returns to idle",
                phase: Phase::Downloading(downloading("b", 18)),
                event: Event::VerifyRefused {
                    sha: sha("b"),
                    reason: "sha256_mismatch".into(),
                },
                expect: (
                    idle("a", Some("z"), 17),
                    vec![
                        Command::Persist(idle("a", Some("z"), 17)),
                        Command::Banner(UpdateBanner::VerifyRefused {
                            target: sha("b"),
                            reason: "sha256_mismatch".into(),
                        }),
                    ],
                ),
            },
            Case {
                name: "downloading boot abandons the download",
                phase: Phase::Downloading(downloading("b", 18)),
                event: Event::Boot,
                expect: (
                    idle("a", Some("z"), 17),
                    vec![Command::Persist(idle("a", Some("z"), 17))],
                ),
            },
            Case {
                name: "downloading tick is nothing",
                phase: Phase::Downloading(downloading("b", 18)),
                event: Event::Tick,
                expect: (Phase::Downloading(downloading("b", 18)), vec![]),
            },
            // ---- Staged
            Case {
                name: "staged boot qualifies",
                phase: Phase::Staged(staged("a", "b")),
                event: Event::Boot,
                expect: (
                    Phase::Staged(staged("a", "b")),
                    vec![Command::Qualify(sha("b"))],
                ),
            },
            Case {
                name: "staged qualify passed flips crash-safely",
                phase: Phase::Staged(staged("a", "b")),
                event: Event::QualifyPassed(sha("b")),
                expect: flip_commands("a", "b", 18),
            },
            Case {
                name: "staged qualify passed for another sha is stale",
                phase: Phase::Staged(staged("a", "b")),
                event: Event::QualifyPassed(sha("q")),
                expect: (Phase::Staged(staged("a", "b")), vec![]),
            },
            Case {
                name: "staged qualify failed stays staged, persists and names the reason",
                phase: Phase::Staged(staged("a", "b")),
                event: Event::QualifyFailed {
                    sha: sha("b"),
                    reason: "codesign_invalid".into(),
                },
                expect: (
                    refused("a", "b", "codesign_invalid"),
                    vec![
                        Command::Persist(refused("a", "b", "codesign_invalid")),
                        Command::Banner(UpdateBanner::QualifyFailed {
                            staged: sha("b"),
                            reason: "codesign_invalid".into(),
                        }),
                    ],
                ),
            },
            Case {
                name: "staged qualify failed for another sha is stale",
                phase: Phase::Staged(staged("a", "b")),
                event: Event::QualifyFailed {
                    sha: sha("q"),
                    reason: "codesign_invalid".into(),
                },
                expect: (Phase::Staged(staged("a", "b")), vec![]),
            },
            Case {
                name: "staged boot of a refused release qualifies it again",
                phase: refused("a", "b", "codesign_invalid"),
                event: Event::Boot,
                expect: (
                    refused("a", "b", "codesign_invalid"),
                    vec![Command::Qualify(sha("b"))],
                ),
            },
            Case {
                name: "staged tick fetches",
                phase: Phase::Staged(staged("a", "b")),
                event: Event::Tick,
                expect: (Phase::Staged(staged("a", "b")), vec![Command::Fetch]),
            },
            Case {
                name: "staged newer manifest discards the staged release and downloads",
                phase: refused("a", "b", "codesign_invalid"),
                event: fetched(19, &host(), "c"),
                expect: (
                    Phase::Downloading(Downloading {
                        target: sha("c"),
                        sequence: 19,
                        ..accepted.clone()
                    }),
                    vec![
                        Command::Persist(idle("a", Some("z"), 17)),
                        Command::Gc {
                            keep: vec![sha("a"), sha("z")],
                        },
                        Command::Persist(Phase::Downloading(Downloading {
                            target: sha("c"),
                            sequence: 19,
                            ..accepted.clone()
                        })),
                        Command::Download {
                            sha: sha("c"),
                            size: 42,
                        },
                    ],
                ),
            },
            Case {
                name: "staged newer manifest naming what runs discards the staged release",
                phase: Phase::Staged(staged("a", "b")),
                event: fetched(19, &host(), "a"),
                expect: (
                    idle("a", Some("z"), 17),
                    vec![
                        Command::Persist(idle("a", Some("z"), 17)),
                        Command::Gc {
                            keep: vec![sha("a"), sha("z")],
                        },
                        Command::Banner(UpdateBanner::UpToDate),
                    ],
                ),
            },
            Case {
                name: "staged same sequence downloads nothing and repeats no banner",
                phase: Phase::Staged(staged("a", "b")),
                event: fetched(18, &host(), "b"),
                expect: (Phase::Staged(staged("a", "b")), vec![]),
            },
            Case {
                name: "staged same sequence of a refused release repeats no banner",
                phase: refused("a", "b", "codesign_invalid"),
                event: fetched(18, &host(), "b"),
                expect: (refused("a", "b", "codesign_invalid"), vec![]),
            },
            Case {
                name: "staged lower sequence changes nothing and repeats no banner",
                phase: Phase::Staged(staged("a", "b")),
                event: fetched(17, &host(), "c"),
                expect: (Phase::Staged(staged("a", "b")), vec![]),
            },
            Case {
                name: "staged refused manifest is a banner",
                phase: Phase::Staged(staged("a", "b")),
                event: Event::ManifestFetched(Err(Refusal::BadSignature)),
                expect: (
                    Phase::Staged(staged("a", "b")),
                    vec![Command::Banner(UpdateBanner::Refused(
                        Refusal::BadSignature,
                    ))],
                ),
            },
            // ---- Swapping
            Case {
                name: "swapping boot asks which side landed",
                phase: Phase::Swapping(swapping("a", "b")),
                event: Event::Boot,
                expect: (
                    Phase::Swapping(swapping("a", "b")),
                    vec![Command::ResolveSwap {
                        from: sha("a"),
                        to: sha("b"),
                    }],
                ),
            },
            Case {
                name: "swapping landed finishes without flipping again",
                phase: Phase::Swapping(swapping("a", "b")),
                event: Event::SwapResolved(SwapState::Landed),
                expect: (
                    pending("b", "a", 0),
                    vec![
                        Command::Persist(pending("b", "a", 0)),
                        Command::Exec(sha("b")),
                    ],
                ),
            },
            Case {
                name: "swapping untouched flips then finishes",
                phase: Phase::Swapping(swapping("a", "b")),
                event: Event::SwapResolved(SwapState::Untouched),
                expect: (
                    pending("b", "a", 0),
                    vec![
                        Command::Flip {
                            from: sha("a"),
                            to: sha("b"),
                        },
                        Command::Persist(pending("b", "a", 0)),
                        Command::Exec(sha("b")),
                    ],
                ),
            },
            // ---- PendingHealthy
            Case {
                name: "pending first boot, the exec'd image's own, counts",
                phase: pending("b", "a", 0),
                event: Event::Boot,
                expect: (
                    pending("b", "a", 1),
                    vec![Command::Persist(pending("b", "a", 1))],
                ),
            },
            Case {
                name: "pending boot after that rolls back crash-safely",
                phase: pending("b", "a", 1),
                event: Event::Boot,
                expect: (
                    rolled_back("a", "b"),
                    vec![
                        Command::Persist(Phase::Swapping(swapping("b", "a"))),
                        Command::Flip {
                            from: sha("b"),
                            to: sha("a"),
                        },
                        Command::Persist(rolled_back("a", "b")),
                        Command::Exec(sha("a")),
                    ],
                ),
            },
            Case {
                name: "pending rendered is healthy and collects garbage",
                phase: pending("b", "a", 1),
                event: Event::Rendered,
                expect: (
                    idle("b", Some("a"), 18),
                    vec![
                        Command::Persist(idle("b", Some("a"), 18)),
                        Command::Gc {
                            keep: vec![sha("b"), sha("a")],
                        },
                    ],
                ),
            },
            Case {
                name: "pending tick does not fetch",
                phase: pending("b", "a", 0),
                event: Event::Tick,
                expect: (pending("b", "a", 0), vec![]),
            },
            // ---- RolledBack
            Case {
                name: "rolled back dismiss keeps the failed release as previous",
                phase: rolled_back("a", "b"),
                event: Event::DismissRollbackNotice,
                expect: (
                    idle("a", Some("b"), 18),
                    vec![Command::Persist(idle("a", Some("b"), 18))],
                ),
            },
            Case {
                name: "rolled back boot runs current by the executor rule",
                phase: rolled_back("a", "b"),
                event: Event::Boot,
                expect: (rolled_back("a", "b"), vec![]),
            },
            Case {
                name: "rolled back rendered keeps the notice",
                phase: rolled_back("a", "b"),
                event: Event::Rendered,
                expect: (rolled_back("a", "b"), vec![]),
            },
            // ---- the channel switched
            Case {
                name: "a switch while idle changes nothing: the next check asks the new channel",
                phase: idle("a", Some("z"), 17),
                event: Event::ChannelChanged,
                expect: (idle("a", Some("z"), 17), vec![]),
            },
            Case {
                name: "a switch mid-download drops the old channel's offer",
                phase: Phase::Downloading(downloading("b", 18)),
                event: Event::ChannelChanged,
                expect: (
                    idle("a", Some("z"), 17),
                    vec![
                        Command::Persist(idle("a", Some("z"), 17)),
                        Command::Gc {
                            keep: vec![sha("a"), sha("z")],
                        },
                    ],
                ),
            },
            Case {
                name: "a switch drops the old channel's staged release, the pin kept",
                phase: Phase::Staged(staged("a", "b")),
                event: Event::ChannelChanged,
                expect: (
                    idle("a", Some("z"), 17),
                    vec![
                        Command::Persist(idle("a", Some("z"), 17)),
                        Command::Gc {
                            keep: vec![sha("a"), sha("z")],
                        },
                    ],
                ),
            },
            Case {
                name: "a switch drops a staged release that failed to qualify too",
                phase: refused("a", "b", "qualify_exit_1"),
                event: Event::ChannelChanged,
                expect: (
                    idle("a", Some("z"), 17),
                    vec![
                        Command::Persist(idle("a", Some("z"), 17)),
                        Command::Gc {
                            keep: vec![sha("a"), sha("z")],
                        },
                    ],
                ),
            },
            Case {
                name: "a switch leaves a flip waiting for its first frame alone",
                phase: pending("b", "a", 1),
                event: Event::ChannelChanged,
                expect: (pending("b", "a", 1), vec![]),
            },
        ]
    }

    #[test]
    fn transitions() {
        for case in table() {
            let got = step(case.phase, case.event);
            assert_eq!(got, case.expect, "{}", case.name);
        }
    }

    /// The phase `step` returns is the phase the executor ends up holding:
    /// equal to the last `Persist` in the list when there is one.
    #[test]
    fn returned_phase_is_the_last_persisted_one() {
        for case in table() {
            let (phase, commands) = step(case.phase, case.event);
            let last_persisted = commands.iter().rev().find_map(|command| match command {
                Command::Persist(persisted) => Some(persisted),
                _ => None,
            });
            if let Some(persisted) = last_persisted {
                assert_eq!(&phase, persisted, "{}", case.name);
            }
        }
    }

    /// A Flip is always followed by an Exec in the same list (an Exec alone
    /// is the resumed swap that had already landed), and the pin never
    /// lowers.
    #[test]
    fn flip_is_followed_by_exec_and_the_pin_never_lowers() {
        for case in table() {
            let before = case.phase.pinned_sequence();
            let (phase, commands) = step(case.phase, case.event);
            assert!(phase.pinned_sequence() >= before, "{}", case.name);
            let flip_at = commands
                .iter()
                .position(|c| matches!(c, Command::Flip { .. }));
            let exec_at = commands.iter().position(|c| matches!(c, Command::Exec(_)));
            match (flip_at, exec_at) {
                (Some(flip), Some(exec)) => assert!(flip < exec, "{}", case.name),
                (Some(flip), None) => panic!("{}: flip at {flip} without exec", case.name),
                (None, Some(_)) | (None, None) => {}
            }
        }
    }

    /// Verified with an announced successor records it before the phase.
    #[test]
    fn verified_pins_the_announced_successor() {
        use crate::manifest::SuccessorKey;
        use crate::release::{PublicKey, testkit::key_pair};
        let successor = SuccessorKey {
            pubkey: PublicKey::of(&key_pair(6)),
            from_sequence: 20,
        };
        let mut d = downloading("b", 18);
        d.successor_key = Some(successor.clone());
        let (_, commands) = step(Phase::Downloading(d), Event::Verified(sha("b")));
        assert_eq!(commands[0], Command::PinSuccessor(successor));
        assert!(matches!(commands[1], Command::Persist(Phase::Staged(_))));
    }

    /// A staged release whose own qualify refuses it does not spend the
    /// channel. It stays staged, but a check still fetches, a newer sequence
    /// supersedes it, and its own sequence is not downloaded again.
    #[test]
    fn a_refused_staged_release_does_not_freeze_the_channel() {
        let refusal = Event::QualifyFailed {
            sha: sha("d"),
            reason: "qualify_exit_3".into(),
        };
        let (refused, _) = step(Phase::Staged(staged("a", "d")), refusal);

        let (refused, commands) = step(refused, Event::Tick);
        assert_eq!(
            commands,
            vec![Command::Fetch],
            "a refused release still checks"
        );

        // a newer good release supersedes it: the staged one is collected,
        // the newer one downloaded
        let (next, commands) = step(refused.clone(), fetched(19, &host(), "e"));
        let Phase::Downloading(downloading) = &next else {
            panic!("a newer sequence is downloaded, not {next:?}");
        };
        assert_eq!((downloading.target, downloading.sequence), (sha("e"), 19));
        let collected = Command::Gc {
            keep: vec![sha("a"), sha("z")],
        };
        assert!(commands.contains(&collected), "{commands:?}");
        assert_eq!(
            commands.last(),
            Some(&Command::Download {
                sha: sha("e"),
                size: 42
            })
        );

        // its own sequence again: no download and no repeated banner; why it
        // waits is read from the phase
        let (same, commands) = step(refused.clone(), fetched(18, &host(), "d"));
        assert_eq!(same, refused);
        assert_eq!(commands, vec![], "a standing release repeats nothing");
        let Phase::Staged(standing) = &same else {
            panic!("staged, not {same:?}");
        };
        assert_eq!(standing.refused.as_deref(), Some("qualify_exit_3"));
    }

    /// The refusal happens in the boot step, which then runs the current
    /// release: the reason rides `state.json`, so the app reads it from the
    /// phase. The qualify banners it once; the app's checks repeat nothing.
    #[test]
    fn a_qualify_refusal_reaches_the_app_through_state_json() {
        let (booted, commands) = step(Phase::Staged(staged("a", "d")), Event::Boot);
        assert_eq!(commands, vec![Command::Qualify(sha("d"))]);
        let refusal = Event::QualifyFailed {
            sha: sha("d"),
            reason: "qualify_exit_3".into(),
        };
        let (_, commands) = step(booted, refusal);
        let Some(Command::Persist(written)) = commands.first() else {
            panic!("the boot step persists the refusal: {commands:?}");
        };
        let banners = commands
            .iter()
            .filter(|command| matches!(command, Command::Banner(_)))
            .count();
        assert_eq!(banners, 1, "the qualify says it once: {commands:?}");

        let read = crate::state::decode(&crate::state::encode(written)).unwrap();
        let Phase::Staged(app_sees) = &read else {
            panic!("the app reads a staged phase, not {read:?}");
        };
        assert_eq!(app_sees.refused.as_deref(), Some("qualify_exit_3"));

        let (read, commands) = step(read, Event::Tick);
        assert_eq!(commands, vec![Command::Fetch]);
        let (read, commands) = step(read, fetched(18, &host(), "d"));
        assert_eq!(commands, vec![], "the app's check repeats no banner");
        assert_eq!(read, *written, "and the reason stays readable");
    }

    /// THE ROLLBACK, as the boots happen: the flip execs the new release,
    /// whose own boot step counts it; it never renders, and the next start
    /// flips back and execs the previous release, which then runs its own
    /// boot step on the notice and stays.
    #[test]
    fn a_flipped_release_that_never_renders_is_rolled_back_at_the_next_start() {
        let (flipped, commands) = step(
            Phase::Staged(staged("a", "b")),
            Event::QualifyPassed(sha("b")),
        );
        assert_eq!(commands.last(), Some(&Command::Exec(sha("b"))));
        let (counted, commands) = step(flipped, Event::Boot);
        assert_eq!(counted, pending("b", "a", 1));
        assert!(
            !commands.iter().any(|c| matches!(c, Command::Exec(_))),
            "{commands:?}"
        );

        let (rolled, commands) = step(counted, Event::Boot);
        assert_eq!(rolled, rolled_back("a", "b"));
        assert_eq!(commands.last(), Some(&Command::Exec(sha("a"))));
        let (stays, commands) = step(rolled, Event::Boot);
        assert_eq!((stays, commands), (rolled_back("a", "b"), vec![]));

        // and the healthy path: a first frame before that next start keeps
        // the new release and collects the rest.
        let (healthy, commands) = step(pending("b", "a", 1), Event::Rendered);
        assert_eq!(healthy, idle("b", Some("a"), 18));
        assert_eq!(
            commands.last(),
            Some(&Command::Gc {
                keep: vec![sha("b"), sha("a")]
            })
        );
    }

    /// Every (Phase, Event) pair is routed: exhaustiveness is the compiler's,
    /// but a stale pair must also be a no-op, never a panic.
    #[test]
    fn every_pair_is_total() {
        let phases = vec![
            idle("a", Some("z"), 17),
            Phase::Downloading(downloading("b", 18)),
            Phase::Staged(staged("a", "b")),
            Phase::Swapping(swapping("a", "b")),
            pending("b", "a", 0),
            rolled_back("a", "b"),
        ];
        let events = vec![
            Event::Boot,
            Event::SwapResolved(SwapState::Landed),
            Event::ManifestFetched(Err(Refusal::BadSignature)),
            fetched(19, &host(), "c"),
            Event::DownloadFinished { sha: sha("b") },
            Event::DownloadFailed {
                sha: sha("b"),
                reason: "x".into(),
            },
            Event::Verified(sha("b")),
            Event::VerifyRefused {
                sha: sha("b"),
                reason: "x".into(),
            },
            Event::QualifyPassed(sha("b")),
            Event::QualifyFailed {
                sha: sha("b"),
                reason: "x".into(),
            },
            Event::DismissRollbackNotice,
            Event::Rendered,
            Event::Tick,
            Event::ChannelChanged,
        ];
        for phase in &phases {
            for event in &events {
                let (next, _) = step(phase.clone(), event.clone());
                assert!(next.pinned_sequence() >= phase.pinned_sequence());
            }
        }
    }

    /// The shape of `step` is load-bearing: one `match`, no wildcard arm,
    /// every arm a single delegation. A source-parsing lint, not a comment.
    #[test]
    fn step_is_one_match_with_no_wildcard_and_only_delegating_arms() {
        let source = include_str!("step.rs");
        let start = source.find("pub fn step(").expect("step is defined here");
        let body_start = start + source[start..].find('{').expect("a body");
        let body = &source[body_start..body_start + brace_span(&source[body_start..])];
        let code: String = body
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(code.matches("match ").count(), 1, "one match in step");
        assert!(!code.contains("_ =>"), "no wildcard arm in step");
        assert!(!code.contains("if "), "no match guard or branch in step");
        for arm in code.split("=>").skip(1) {
            let target = arm.trim_start();
            let delegates =
                target.starts_with(|c: char| c.is_ascii_lowercase()) || target.starts_with("{\n");
            assert!(delegates, "arm body must be a single call: {target:.40}");
        }
    }

    fn brace_span(text: &str) -> usize {
        let mut depth = 0usize;
        for (index, character) in text.char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return index + 1;
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces");
    }
}
