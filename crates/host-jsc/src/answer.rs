//! What the page's answers mean (`glue.js` writes them): one string per
//! call, read here into what lands on the seat, with the same meaning the
//! wasmtime runner gives the same view's answers. The page holds no meaning
//! of its own; this file is all of it, and builds and is tested everywhere.

use crate::VIEW_FUEL;
use base64::Engine as _;
use ducktape_host::contract::{Fault, Landed};
use view_meter::Stop;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) fuel_used: u64,
    pub(crate) memory: Option<usize>,
    pub(crate) outcome: Result<Vec<u8>, Fault>,
}

pub(crate) fn read(text: &str) -> Answer {
    let failed = |fault: Fault, memory: Option<usize>| Answer {
        fuel_used: 0,
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
            Answer {
                fuel_used: (VIEW_FUEL - gas.max(0)).unsigned_abs(),
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
                Stop::Memory => Fault::Memory,
                Stop::OutOfFuel | Stop::Trap => {
                    Fault::Trap(fields.next().unwrap_or("trap").to_owned())
                }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Export {
    Init,
    Tick,
    Snapshot,
    Restore,
}

impl Export {
    pub(crate) fn op(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Tick => "tick",
            Self::Snapshot => "snapshot",
            Self::Restore => "restore",
        }
    }

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
    fn a_returned_call_used_the_full_count_less_what_it_left() {
        let answer = read(&format!("R {} 65536 {}", VIEW_FUEL - 7, b64(b"frame")));
        assert_eq!(answer.fuel_used, 7);
        assert_eq!(answer.memory, Some(65_536));
        assert_eq!(answer.outcome, Ok(b"frame".to_vec()));
        assert_eq!(read("R -12 0 ").fuel_used, VIEW_FUEL.unsigned_abs());
        assert_eq!(read("R -12 0 ").outcome, Ok(Vec::new()));
    }

    #[test]
    fn a_trap_is_read_off_gas_left_as_the_meter_leaves_it() {
        let trapped =
            |gas: i64| read(&format!("T {gas} 131072 RuntimeError: Out of bounds")).outcome;
        let out_of_bounds = Err(Fault::Trap("RuntimeError: Out of bounds".into()));
        assert_eq!(trapped(0), out_of_bounds);
        assert_eq!(trapped(-3), out_of_bounds);
        assert_eq!(trapped(view_meter::GROW_REFUSED), Err(Fault::Memory));
        assert_eq!(
            trapped(42),
            Err(Fault::Trap("RuntimeError: Out of bounds".into()))
        );
        let answer = read("T 42 131072 unreachable");
        assert_eq!((answer.fuel_used, answer.memory), (0, Some(131_072)));
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
            Export::Init.landed(Err(Fault::Memory)),
            Landed::Fault(Fault::Memory)
        ));
    }
}
