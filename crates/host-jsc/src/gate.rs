//! The runner equivalence gate's session driver (SPEC §13.3 (b)): a
//! recorded session (S0-webkit's chat and forge recordings, the view
//! wasm they were made with) driven through any `ViewRunner` the way the
//! host drives a seat: an instance, `init`, a tick per recorded batch,
//! each answer awaited as it lands. The simulator gate (`examples/
//! sim_gate.rs`) runs it on [`crate::JscRunner`]; this crate's tests run it
//! on wasmtime, which pins the numbers both must give: the fuel over the
//! session, every frame equal to the recorded one, the frame hash after a
//! restore, and the frames after a death replay.
//!
//! The driver blocks its thread on each landing: it runs on a thread of its
//! own, never the main thread (where JSC's answers land).

use ducktape_host::contract::{Fault, Landed, Landing, SeatId, ViewInstance, ViewRunner};
use sha2::Digest as _;
use std::io::Read as _;
use std::time::{Duration, Instant};

/// The view memory every session runs under: the setting's default.
pub const VIEW_BYTES: usize = 64 << 20;

/// One recorded session and what the wasmtime reference gives for it.
pub struct Session {
    /// The view's name.
    pub view: &'static str,
    /// The view's wasm.
    pub code: Vec<u8>,
    /// Per tick: the event batch, and the frame the view answered.
    pub ticks: Vec<(Vec<u8>, Vec<u8>)>,
    /// The fuel the whole session uses on wasmtime.
    pub fuel: u64,
    /// The tick a restore crosscheck snapshots after.
    pub snapshot_at: usize,
    /// FNV-1a 64 over the frames after that restore (S0-webkit's
    /// `replay.js restore`).
    pub restored: &'static str,
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let read = flate2::read::GzDecoder::new(bytes).read_to_end(&mut out);
    debug_assert!(read.is_ok(), "a committed recording unzips");
    out
}

/// The recording's ticks: u8 rendered, u32le len + the event batch, u32le
/// len + the frame.
fn ticks(bytes: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut at = 0;
    let chunk = |at: &mut usize| {
        let len = bytes
            .get(*at..*at + 4)
            .and_then(|len| <[u8; 4]>::try_from(len).ok())
            .map_or(0, u32::from_le_bytes) as usize;
        *at += 4;
        let out = bytes.get(*at..*at + len).unwrap_or_default().to_vec();
        *at += len;
        out
    };
    let mut ticks = Vec::new();
    while at < bytes.len() {
        at += 1;
        let events = chunk(&mut at);
        let frame = chunk(&mut at);
        ticks.push((events, frame));
    }
    ticks
}

/// The two recorded sessions, with the wasmtime reference's numbers.
pub fn sessions() -> [Session; 2] {
    [
        Session {
            view: "chat",
            code: gunzip(include_bytes!(
                "../../view-meter/tests/sessions/chat_view.wasm.gz"
            )),
            ticks: ticks(&gunzip(include_bytes!(
                "../../view-meter/tests/sessions/chat-desk.rec.gz"
            ))),
            fuel: 1_906_273_787,
            snapshot_at: 119,
            restored: "bf0bc34cfbd3be03",
        },
        Session {
            view: "forge",
            code: gunzip(include_bytes!(
                "../../view-meter/tests/sessions/forge_view.wasm.gz"
            )),
            ticks: ticks(&gunzip(include_bytes!(
                "../../view-meter/tests/sessions/forge-desk.rec.gz"
            ))),
            fuel: 1_325_536_710,
            snapshot_at: 92,
            restored: "13577549968c5b26",
        },
    ]
}

/// FNV-1a 64 over each frame's u32le length and bytes.
fn fnv<'a>(frames: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for frame in frames {
        let len = u32::try_from(frame.len()).unwrap_or(u32::MAX).to_le_bytes();
        for byte in len.iter().chain(frame) {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// A seat as the host drives one: its instance and the landing it answers on.
struct Seat {
    instance: Box<dyn ViewInstance>,
    landing: async_channel::Receiver<Landed>,
}

/// The next answer, waited for.
fn next(landing: &async_channel::Receiver<Landed>) -> Landed {
    landing
        .recv_blocking()
        .unwrap_or_else(|_| Landed::Fault(Fault::Runner("the landing closed".into())))
}

fn seat(runner: &dyn ViewRunner, hash: [u8; 32], instance: u64) -> Result<Seat, String> {
    let (land, landing) = Landing::channel();
    runner.instantiate(hash, SeatId("gate", instance), land);
    match next(&landing) {
        Landed::Seated(instance) => Ok(Seat { instance, landing }),
        other => Err(format!("seat: {other:?}")),
    }
}

impl Seat {
    fn init(&mut self) -> Result<(), String> {
        self.instance.begin_init();
        match next(&self.landing) {
            Landed::Inited => Ok(()),
            other => Err(format!("init: {other:?}")),
        }
    }

    fn tick(&mut self, events: &[u8]) -> Landed {
        self.instance.begin_tick(events.to_vec());
        next(&self.landing)
    }

    fn frame(&mut self, events: &[u8]) -> Result<Vec<u8>, String> {
        match self.tick(events) {
            Landed::Frame(frame) => Ok(frame),
            other => Err(format!("tick: {other:?}")),
        }
    }

    fn snapshot(&mut self) -> Result<Vec<u8>, String> {
        self.instance.begin_snapshot();
        match next(&self.landing) {
            Landed::Snapshot(state) => Ok(state),
            other => Err(format!("snapshot: {other:?}")),
        }
    }

    fn restore(&mut self, state: &[u8]) -> Result<(), String> {
        self.instance.begin_restore(state.to_vec());
        match next(&self.landing) {
            Landed::Restored => Ok(()),
            other => Err(format!("restore: {other:?}")),
        }
    }
}

fn hash_of(code: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(code).into()
}

/// What one session gave on a runner.
#[derive(Debug, PartialEq, Eq)]
pub struct Ran {
    /// The fuel over every tick.
    pub fuel: u64,
    /// How many ticks answered the recorded frame, of how many.
    pub frames: (usize, usize),
    /// The frame hash after the restore crosscheck.
    pub restored: String,
    /// Each tick's wall time from its call to its landing, sorted.
    pub ticks: Vec<Duration>,
}

/// `session` on `runner`: the whole recording on one instance, then the
/// restore crosscheck (to `snapshot_at`, a snapshot, a fresh instance
/// restored from it, the rest replayed and hashed).
pub fn run(runner: &dyn ViewRunner, session: &Session) -> Result<Ran, String> {
    let hash = hash_of(&session.code);
    let _held = runner
        .prepare(hash, &session.code, VIEW_BYTES)
        .map_err(|failure| format!("prepare: {failure}"))?;
    let mut whole = seat(runner, hash, 1)?;
    whole.init()?;
    let (mut fuel, mut equal, mut ticks) = (0, 0, Vec::new());
    for (events, recorded) in &session.ticks {
        let began = Instant::now();
        let frame = whole.frame(events)?;
        ticks.push(began.elapsed());
        fuel += whole.instance.fuel_used();
        equal += usize::from(frame == *recorded);
    }
    ticks.sort();
    let k = session.snapshot_at;
    let mut first = seat(runner, hash, 2)?;
    first.init()?;
    for (events, _) in &session.ticks[..=k] {
        first.frame(events)?;
    }
    let state = first.snapshot()?;
    let mut fresh = seat(runner, hash, 3)?;
    fresh.restore(&state)?;
    let mut after = Vec::new();
    for (events, _) in &session.ticks[k + 1..] {
        after.push(fresh.frame(events)?);
    }
    Ok(Ran {
        fuel,
        frames: (equal, session.ticks.len()),
        restored: fnv(after.iter().map(Vec::as_slice)),
        ticks,
    })
}

/// What a death and the host's revive gave.
#[derive(Debug, PartialEq, Eq)]
pub struct Revived {
    /// What the tick begun on the dead instance landed.
    pub dead_tick: String,
    /// Ticks after the death that answered the recorded frame, of how many.
    pub frames: (usize, usize),
    /// The fuel over those ticks, and the reference run's over the same.
    pub fuel: (u64, u64),
}

/// A death and the host's revive (contract `ViewInstance`, SPEC §6.5): to
/// `snapshot_at`, a snapshot kept; ten more ticks, their batches kept; then
/// `kill` (which returns once the page is dead; `None`: no death, as on
/// wasmtime, where the same revive runs on a living view); a tick begun on
/// the old instance (on a dead page it lands `Fault::Runner` at once); a new
/// instance seated at once (right after a death it waits for the new page),
/// by the code only the dead instance still holds,
/// restored from the kept snapshot, the kept batches replayed, and the rest
/// of the session ticked on it, every frame against the recorded one and
/// the fuel against `reference`'s, the per-tick fuel of an uninterrupted run.
pub fn revive(
    runner: &dyn ViewRunner,
    session: &Session,
    reference: &[u64],
    kill: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<Revived, String> {
    let hash = hash_of(&session.code);
    let held = runner
        .prepare(hash, &session.code, VIEW_BYTES)
        .map_err(|failure| format!("prepare: {failure}"))?;
    let k = session.snapshot_at;
    let last_kept = (k + 10).min(session.ticks.len() - 2);
    let mut old = seat(runner, hash, 4)?;
    old.init()?;
    for (events, _) in &session.ticks[..=k] {
        old.frame(events)?;
    }
    let state = old.snapshot()?;
    for (events, _) in &session.ticks[k + 1..=last_kept] {
        old.frame(events)?;
    }
    // from here only the instance holds its code, as a drawn view's does
    drop(held);
    if let Some(kill) = kill {
        kill()?;
    }
    let died = &session.ticks[last_kept + 1];
    let dead_tick = match old.tick(&died.0) {
        Landed::Frame(_) => "Frame".to_owned(),
        other => format!("{other:?}"),
    };
    // the host's revive: a new instance at once, its code still held by
    // the dead one
    let mut new = seat(runner, hash, 5)?;
    drop(old);
    new.restore(&state)?;
    for (events, _) in &session.ticks[k + 1..=last_kept] {
        new.frame(events)?;
    }
    let (mut equal, mut fuel, mut expected) = (0, 0, 0);
    let rest = &session.ticks[last_kept + 1..];
    for (at, (events, recorded)) in rest.iter().enumerate() {
        let frame = new.frame(events)?;
        equal += usize::from(frame == *recorded);
        fuel += new.instance.fuel_used();
        expected += reference.get(last_kept + 1 + at).copied().unwrap_or(0);
    }
    Ok(Revived {
        dead_tick,
        frames: (equal, rest.len()),
        fuel: (fuel, expected),
    })
}

/// Each tick's fuel over the whole session on one instance: the reference
/// a revive's fuel is held to.
pub fn fuel_per_tick(runner: &dyn ViewRunner, session: &Session) -> Result<Vec<u64>, String> {
    let hash = hash_of(&session.code);
    let _held = runner
        .prepare(hash, &session.code, VIEW_BYTES)
        .map_err(|failure| format!("prepare: {failure}"))?;
    let mut whole = seat(runner, hash, 6)?;
    whole.init()?;
    session
        .ticks
        .iter()
        .map(|(events, _)| {
            whole.frame(events)?;
            Ok(whole.instance.fuel_used())
        })
        .collect()
}

// wasmtime is the reference, and no wasmtime is linked on iOS
#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use super::*;
    use ducktape_host::contract::RunnerConfig;
    use ducktape_host::runner::WasmtimeRunner;

    fn wasmtime() -> WasmtimeRunner {
        WasmtimeRunner::new(RunnerConfig {
            signals_based_traps: true,
            cache_dir: None,
        })
    }

    /// The numbers the simulator gate must give, on the desktop's runner:
    /// the fuel the meter pass was proven against, every recorded frame,
    /// the S0 restore hashes, and a revive that reproduces every frame and
    /// tick's fuel after it.
    #[test]
    fn the_sessions_give_the_reference_numbers_on_wasmtime() {
        let runner = wasmtime();
        for session in sessions() {
            let ran = run(&runner, &session).unwrap();
            let all = session.ticks.len();
            assert_eq!(ran.fuel, session.fuel, "{}: fuel", session.view);
            assert_eq!(ran.frames, (all, all), "{}: frames", session.view);
            assert_eq!(ran.restored, session.restored, "{}: restore", session.view);
            let reference = fuel_per_tick(&runner, &session).unwrap();
            assert_eq!(reference.iter().sum::<u64>(), session.fuel);
            let revived = revive(&runner, &session, &reference, None).unwrap();
            assert_eq!(revived.dead_tick, "Frame", "no death on wasmtime");
            assert_eq!(
                revived.frames.0, revived.frames.1,
                "{}: revive",
                session.view
            );
            assert_eq!(
                revived.fuel.0, revived.fuel.1,
                "{}: revive fuel",
                session.view
            );
        }
    }
}
