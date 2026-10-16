//! What the page's answers mean (`glue.js` writes them): one string per
//! call, read here into what lands on the seat, with the same meaning the
//! wasmtime runner gives the same view's answers. The page holds no meaning
//! of its own; this file is all of it, and builds and is tested everywhere.

use base64::Engine as _;
use ducktape_host::contract::{FUEL_PER_TICK, Fault, Landed};
use view_meter::Stop;

/// One call's answer, read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    /// The fuel the call used, of [`FUEL_PER_TICK`]: a call that returned
    /// used the budget less what it left, capped at the budget (a call can
    /// run past it between two checks and return, as on wasmtime, whose
    /// count then reads the whole budget); a call that failed reports the
    /// whole budget.
    pub(crate) fuel_used: u64,
    /// The view's linear memory after the call, when the page said.
    pub(crate) memory: Option<usize>,
    /// The bytes the export answered, or why there are none.
    pub(crate) outcome: Result<Vec<u8>, Fault>,
}

/// Reads the page's answer `text`.
pub(crate) fn read(text: &str) -> Answer {
    let failed = |fault: Fault, memory: Option<usize>| Answer {
        fuel_used: FUEL_PER_TICK,
        memory,
        outcome: Err(fault),
    };
    let (kind, rest) = text.split_once(' ').unwrap_or((text, ""));
    match kind {
        "R" => {
            let mut fields = rest.splitn(3, ' ');
            let (Some(gas), Some(memory), Some(bytes)) = (
                fields.next().and_then(|gas| gas.parse::<i64>().ok()),
                fields
                    .next()
                    .and_then(|memory| memory.parse::<usize>().ok()),
                fields
                    .next()
                    .and_then(|bytes| base64::engine::general_purpose::STANDARD.decode(bytes).ok()),
            ) else {
                return failed(Fault::Runner(format!("the page answered {text:.80}")), None);
            };
            let left = u64::try_from(gas).unwrap_or(0).min(FUEL_PER_TICK);
            Answer {
                fuel_used: FUEL_PER_TICK - left,
                memory: Some(memory),
                outcome: Ok(bytes),
            }
        }
        "T" => {
            let mut fields = rest.splitn(3, ' ');
            let (Some(gas), Some(memory)) = (
                fields.next().and_then(|gas| gas.parse::<i64>().ok()),
                fields
                    .next()
                    .and_then(|memory| memory.parse::<usize>().ok()),
            ) else {
                return failed(Fault::Runner(format!("the page answered {text:.80}")), None);
            };
            let fault = match view_meter::stop(gas) {
                Stop::OutOfFuel => Fault::OutOfFuel,
                Stop::Memory => Fault::Memory,
                Stop::Trap => Fault::Trap(fields.next().unwrap_or("trap").to_owned()),
            };
            failed(fault, Some(memory))
        }
        "P" => {
            let (memory, words) = rest.split_once(' ').unwrap_or((rest, ""));
            failed(Fault::Panicked(words.to_owned()), memory.parse().ok())
        }
        "F" => failed(Fault::Trap(rest.to_owned()), None),
        "E" => failed(Fault::Runner(rest.to_owned()), None),
        _ => failed(Fault::Runner(format!("the page answered {text:.80}")), None),
    }
}

/// The export a call entered, for what its bytes mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Export {
    Init,
    Tick,
    Snapshot,
    Restore,
}

impl Export {
    /// The page's name for the call.
    pub(crate) fn op(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Tick => "tick",
            Self::Snapshot => "snapshot",
            Self::Restore => "restore",
        }
    }

    /// What lands for `outcome`: as the wasmtime runner reads the same
    /// export's answer (`snapshot` and `restore` answer a result,
    /// `wire::abi::decode_result`).
    pub(crate) fn landed(self, outcome: Result<Vec<u8>, Fault>) -> Landed {
        let bytes = match outcome {
            Ok(bytes) => bytes,
            Err(fault) => return Landed::Fault(fault),
        };
        let result = || {
            view_wire::abi::decode_result(&bytes)
                .ok_or_else(|| Fault::Trap("the view's answer is not a result".into()))
        };
        match self {
            Self::Init => Landed::Inited,
            Self::Tick => Landed::Frame(bytes),
            Self::Snapshot => match result() {
                Ok(Ok(state)) => Landed::Snapshot(state),
                Ok(Err(refusal)) => Landed::SnapshotRefused(refusal),
                Err(fault) => Landed::Fault(fault),
            },
            Self::Restore => match result() {
                Ok(Ok(_)) => Landed::Restored,
                Ok(Err(refusal)) => Landed::RestoreRefused(refusal),
                Err(fault) => Landed::Fault(fault),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn a_returned_call_used_the_budget_less_what_it_left_at_most_the_budget() {
        let answer = read(&format!("R {} 65536 {}", FUEL_PER_TICK - 7, b64(b"frame")));
        assert_eq!(answer.fuel_used, 7);
        assert_eq!(answer.memory, Some(65_536));
        assert_eq!(answer.outcome, Ok(b"frame".to_vec()));
        // past the budget between two checks, and returned: the whole budget
        assert_eq!(read("R -12 0 ").fuel_used, FUEL_PER_TICK);
        assert_eq!(read("R -12 0 ").outcome, Ok(Vec::new()));
    }

    #[test]
    fn a_trap_is_read_off_gas_left_as_the_meter_leaves_it() {
        let trapped =
            |gas: i64| read(&format!("T {gas} 131072 RuntimeError: Out of bounds")).outcome;
        assert_eq!(trapped(0), Err(Fault::OutOfFuel));
        assert_eq!(trapped(-3), Err(Fault::OutOfFuel));
        assert_eq!(trapped(view_meter::GROW_REFUSED), Err(Fault::Memory));
        assert_eq!(
            trapped(42),
            Err(Fault::Trap("RuntimeError: Out of bounds".into()))
        );
        let answer = read("T 42 131072 unreachable");
        assert_eq!(
            (answer.fuel_used, answer.memory),
            (FUEL_PER_TICK, Some(131_072))
        );
    }

    #[test]
    fn a_panic_a_view_fault_and_the_pages_own_are_told_apart() {
        assert_eq!(
            read("P 65536 panicked at src/lib.rs:3:5: no").outcome,
            Err(Fault::Panicked("panicked at src/lib.rs:3:5: no".into()))
        );
        assert_eq!(
            read("F the view exports no tick").outcome,
            Err(Fault::Trap("the view exports no tick".into()))
        );
        assert_eq!(
            read("E no view is seated as 4").outcome,
            Err(Fault::Runner("no view is seated as 4".into()))
        );
        for garbled in ["", "R", "R x 0 ", "R 1 2 !!", "T 1", "Q 1 2 3"] {
            assert!(
                matches!(read(garbled).outcome, Err(Fault::Runner(_))),
                "{garbled:?}"
            );
        }
    }

    #[test]
    fn snapshot_and_restore_answer_a_result_as_on_wasmtime() {
        let ok = view_wire::abi::encode_result(Ok(b"state".to_vec()));
        let refused = view_wire::abi::encode_result(Err("busy".into()));
        assert!(matches!(
            Export::Snapshot.landed(Ok(ok.clone())),
            Landed::Snapshot(state) if state == b"state"
        ));
        assert!(matches!(
            Export::Snapshot.landed(Ok(refused.clone())),
            Landed::SnapshotRefused(words) if words == "busy"
        ));
        assert!(matches!(Export::Restore.landed(Ok(ok)), Landed::Restored));
        assert!(matches!(
            Export::Restore.landed(Ok(refused)),
            Landed::RestoreRefused(words) if words == "busy"
        ));
        assert!(matches!(
            Export::Snapshot.landed(Ok(b"\x07".to_vec())),
            Landed::Fault(Fault::Trap(_))
        ));
        assert!(matches!(
            Export::Tick.landed(Ok(b"frame".to_vec())),
            Landed::Frame(frame) if frame == b"frame"
        ));
        assert!(matches!(
            Export::Init.landed(Err(Fault::OutOfFuel)),
            Landed::Fault(Fault::OutOfFuel)
        ));
    }
}
