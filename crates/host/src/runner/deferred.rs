//! The landed-later gate's runner (SPEC §13.3): a [`ViewRunner`] that runs
//! view code on the wasmtime runner and lands each answer on the seat's
//! [`Landing`] when the test says: at once (the desktop's order), on a
//! later [`Pump::release_next`] / [`Pump::release_all`] (the next
//! `cx.defer`, or late), or never. What a JSC page does across its
//! process boundary, on Linux, before any simulator sees it. A page that
//! dies ([`Pump::die`]) stays dead: every call into its instance from then
//! on lands the runner's fault, as the contract's dead-page rules say.

use super::WasmtimeRunner;
use crate::contract::{Fault, Landed, Landing, SeatId, ViewInstance, ViewRunner};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// An instance's page is dead.
type Dead = Arc<AtomicBool>;

/// A page's death, as every call into it lands it.
fn death() -> Landed {
    Landed::Fault(Fault::Runner("the page died".into()))
}

/// The answers held back, and whether the next ones are held.
#[derive(Clone, Default)]
pub struct Pump(Arc<Mutex<Held>>);

#[derive(Default)]
struct Held {
    holding: bool,
    /// The page is dead while this is set: every answer lands as the
    /// runner's fault, at once.
    dying: bool,
    /// Each held answer, with the page of the instance it came from (none
    /// for a seating's).
    answers: VecDeque<(Landing, Landed, Option<Dead>)>,
}

impl Pump {
    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// From now on every answer waits for a release.
    pub fn hold(&self) {
        self.lock().holding = true;
    }

    /// From now on every answer lands at once; what is held stays held.
    pub fn at_once(&self) {
        self.lock().holding = false;
    }

    /// What the held answers are, oldest first: `Landed`'s variant names.
    pub fn held_kinds(&self) -> Vec<String> {
        self.lock()
            .answers
            .iter()
            .map(|(_, landed, _)| {
                let shown = format!("{landed:?}");
                shown.split('(').next().unwrap_or_default().to_owned()
            })
            .collect()
    }

    /// How many answers wait.
    pub fn held(&self) -> usize {
        self.lock().answers.len()
    }

    /// Lands the oldest held answer; whether there was one.
    pub fn release_next(&self) -> bool {
        let next = self.lock().answers.pop_front();
        next.map(|(land, landed, _)| land.send(landed)).is_some()
    }

    /// Lands every held answer, oldest first; how many.
    pub fn release_all(&self) -> usize {
        let mut released = 0;
        while self.release_next() {
            released += 1;
        }
        released
    }

    /// The page the held answers came from dies (SPEC §6.5): each lands as
    /// the runner's fault instead, the instances they came from stay dead
    /// (every later call into one lands the runner's fault too), and
    /// answers land at once from now on.
    pub fn die(&self) -> usize {
        let answers = {
            let mut held = self.lock();
            held.holding = false;
            std::mem::take(&mut held.answers)
        };
        let died = answers.len();
        for (land, _, dead) in answers {
            if let Some(dead) = dead {
                dead.store(true, Ordering::Relaxed);
            }
            land.send(death());
        }
        died
    }

    /// While `dying`, the page is dead: every answer from then on lands at
    /// once as the runner's fault, as a call into a page that already died
    /// does; then a new page answers again.
    pub fn dying(&self, dying: bool) {
        self.lock().dying = dying;
    }

    /// Drops every held answer: they never land.
    pub fn drop_all(&self) -> usize {
        std::mem::take(&mut self.lock().answers).len()
    }

    /// Lands `landed`, from the instance whose page `dead` is (none for a
    /// seating), as the test says.
    fn land(&self, land: &Landing, landed: Landed, dead: Option<&Dead>) {
        let mut held = self.lock();
        if held.dying {
            drop(held);
            land.send(death());
            return;
        }
        match held.holding {
            true => held
                .answers
                .push_back((land.clone(), landed, dead.cloned())),
            false => {
                drop(held);
                land.send(landed);
            }
        }
    }
}

/// The wasmtime runner, its answers routed through a [`Pump`].
pub struct DeferredRunner {
    inner: WasmtimeRunner,
    /// The test's handle on what lands when.
    pub pump: Pump,
}

impl Default for DeferredRunner {
    /// Answers at once until the pump is told to hold.
    fn default() -> Self {
        Self {
            inner: WasmtimeRunner::new(crate::contract::RunnerConfig {
                signals_based_traps: true,
                cache_dir: None,
                compile_apart: None,
            }),
            pump: Pump::default(),
        }
    }
}

impl ViewRunner for DeferredRunner {
    fn prepare(
        &self,
        hash: [u8; 32],
        code: crate::contract::Code<'_>,
        view_bytes: usize,
    ) -> Result<crate::contract::Prepared, crate::contract::Failure> {
        self.inner.prepare(hash, code, view_bytes)
    }

    fn instantiate(&self, hash: [u8; 32], seat: SeatId, land: Landing) {
        let (inner_land, inner) = Landing::channel();
        self.inner.instantiate(hash, seat, inner_land);
        let landed = match inner.try_recv() {
            Ok(Landed::Seated(instance)) => Landed::Seated(Box::new(DeferredInstance {
                inner: instance,
                answers: inner,
                land: land.clone(),
                pump: self.pump.clone(),
                dead: Dead::default(),
            })),
            Ok(other) => other,
            Err(_) => unreachable!("the wasmtime runner lands at once"),
        };
        self.pump.land(&land, landed, None);
    }

    fn describe(
        &self,
        hash: [u8; 32],
        module: crate::contract::Code<'_>,
        op: &[u8],
    ) -> Option<view_wire::methods::Description> {
        self.inner.describe(hash, module, op)
    }
}

/// A wasmtime instance whose answers go through the pump, on a page that
/// may die (`dead`).
struct DeferredInstance {
    inner: Box<dyn ViewInstance>,
    answers: async_channel::Receiver<Landed>,
    land: Landing,
    pump: Pump,
    dead: Dead,
}

impl DeferredInstance {
    /// `call` on the wasmtime instance, its answer through the pump; on a
    /// dead page nothing runs and the call lands the runner's fault.
    fn call(&mut self, call: impl FnOnce(&mut dyn ViewInstance)) {
        if self.dead.load(Ordering::Relaxed) {
            self.pump.land(&self.land, death(), Some(&self.dead));
            return;
        }
        call(&mut *self.inner);
        while let Ok(landed) = self.answers.try_recv() {
            self.pump.land(&self.land, landed, Some(&self.dead));
        }
    }
}

impl ViewInstance for DeferredInstance {
    fn code_hash(&self) -> [u8; 32] {
        self.inner.code_hash()
    }

    fn begin_init(&mut self) {
        self.call(|inner| inner.begin_init());
    }

    fn begin_tick(&mut self, events: Vec<u8>) {
        self.call(|inner| inner.begin_tick(events));
    }

    fn begin_snapshot(&mut self) {
        self.call(|inner| inner.begin_snapshot());
    }

    fn begin_restore(&mut self, snapshot: Vec<u8>) {
        self.call(|inner| inner.begin_restore(snapshot));
    }

    fn fuel_used(&self) -> u64 {
        self.inner.fuel_used()
    }

    fn memory_bytes(&self) -> usize {
        self.inner.memory_bytes()
    }

    fn stop(self: Box<Self>) {
        self.inner.stop();
    }

    #[cfg(test)]
    fn memory_for_test(&self) -> &[u8] {
        self.inner.memory_for_test()
    }

    #[cfg(test)]
    fn memory_mut_for_test(&mut self) -> &mut [u8] {
        self.inner.memory_mut_for_test()
    }
}
