//! The `state.json` codec: a `Phase`, whole, as pretty JSON.
//!
//! The file is per-install and rewritten whole (tmp-write + rename) on every
//! `Persist`; it carries no version field. There is nothing to migrate: the
//! binary that reads it is the one that writes it.
//!
//! ```json
//! {
//!   "phase": "idle",
//!   "current": "<sha>",
//!   "previous": null,
//!   "pinned_sequence": 17
//! }
//! ```

use crate::phase::Phase;

/// The bytes to write.
pub fn encode(phase: &Phase) -> String {
    let mut text = serde_json::to_string_pretty(phase).expect("a Phase always serializes");
    text.push('\n');
    text
}

/// The phase a file holds.
pub fn decode(text: &str) -> Result<Phase, serde_json::Error> {
    serde_json::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::SuccessorKey;
    use crate::phase::{
        Downloading, Idle, PendingHealthy, RollbackReason, RolledBack, Staged, Swapping,
    };
    use crate::release::{PublicKey, testkit::key_pair};
    use crate::sha::Sha;

    fn every_phase() -> Vec<Phase> {
        let a = Sha::digest(b"a");
        let b = Sha::digest(b"b");
        vec![
            Phase::Idle(Idle {
                current: a,
                previous: None,
                pinned_sequence: 0,
            }),
            Phase::Idle(Idle {
                current: a,
                previous: Some(b),
                pinned_sequence: 17,
            }),
            Phase::Downloading(Downloading {
                current: a,
                previous: Some(b),
                pinned_sequence: 17,
                target: Sha::digest(b"c"),
                size: 12345,
                sequence: 18,
                display: "0.1.0+abc".into(),
                channel: "stable".into(),
                successor_key: Some(SuccessorKey {
                    pubkey: PublicKey::of(&key_pair(5)),
                    from_sequence: 20,
                }),
                asked: true,
            }),
            Phase::Staged(Staged {
                current: a,
                previous: None,
                pinned_sequence: 18,
                staged: b,
                sequence: 18,
                display: "0.1.0+abc".into(),
                channel: "nightly".into(),
                refused: None,
                asked: true,
            }),
            Phase::Staged(Staged {
                current: a,
                previous: Some(b),
                pinned_sequence: 4,
                staged: Sha::digest(b"d"),
                sequence: 4,
                display: "0.1.0+qualify-fail".into(),
                channel: "stable".into(),
                refused: Some("qualify_exit_3".into()),
                asked: false,
            }),
            Phase::Swapping(Swapping {
                from: a,
                to: b,
                pinned_sequence: 18,
                display: "0.1.0+b".into(),
            }),
            Phase::PendingHealthy(PendingHealthy {
                current: b,
                previous: a,
                boots: 1,
                pinned_sequence: 18,
                display: "0.1.0+b".into(),
            }),
            Phase::RolledBack(RolledBack {
                current: a,
                failed: b,
                reason: RollbackReason::NeverRendered,
                pinned_sequence: 18,
                display: "0.1.0+b".into(),
            }),
        ]
    }

    #[test]
    fn every_phase_round_trips() {
        for phase in every_phase() {
            let text = encode(&phase);
            assert!(text.ends_with('\n'));
            assert_eq!(decode(&text).unwrap(), phase, "{text}");
        }
    }

    #[test]
    fn the_file_is_one_flat_object_tagged_by_phase() {
        let a = Sha::digest(b"a");
        let text = encode(&Phase::Swapping(Swapping {
            from: a,
            to: Sha::digest(b"b"),
            pinned_sequence: 3,
            display: "0.1.0+b".into(),
        }));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["phase"], "swapping");
        assert_eq!(value["from"], a.to_string());
        assert_eq!(value["pinned_sequence"], 3);
        assert!(value.get("version").is_none());
    }

    #[test]
    fn unknown_phase_is_an_error() {
        assert!(decode(r#"{"phase":"warp","current":"00"}"#).is_err());
        assert!(decode("").is_err());
    }
}
