//! The desktop app's self-update, as a pure library.
//!
//! One binary drives one state machine and one file per install: at the top
//! of `ducktape-app`'s `main` the boot step reads `state.json`, runs [`step`]
//! on [`Event::Boot`], performs the flip or rollback it is told and `exec`s
//! the release that should run; while the app runs it checks, downloads,
//! verifies and stages through the same [`step`], and its first frame marks
//! a flipped release healthy.
//!
//! This crate holds the vocabulary ([`Phase`], [`Event`], [`Command`]), the
//! decision ([`step`]), the signed release manifest ([`Manifest`],
//! [`verify_manifest`]), its signature ([`release`]), where a release is
//! published ([`layout`]) and the `state.json` codec ([`state`]). It performs
//! no I/O, reads no clock and opens no socket: every effect is a [`Command`]
//! for the executor.

pub mod layout;
pub mod manifest;
pub mod phase;
pub mod release;
pub mod sha;
pub mod state;
pub mod step;
pub mod verify;

pub use manifest::{Artifact, Manifest, Platform, Release, SCHEMA, SuccessorKey};
pub use phase::{
    Command, Downloading, Event, Idle, PendingHealthy, Phase, RollbackReason, RolledBack, Staged,
    SwapState, Swapping, Take, UpdateBanner,
};
pub use release::{PublicKey, RELEASE_KEY, RELEASE_NS, Signature};
pub use sha::Sha;
pub use step::step;
pub use verify::{Refusal, SignedManifest, TrustedKeys, VerifiedManifest, verify_manifest};
