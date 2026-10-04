//! The wasm side of a view: the store state the host keeps for it and
//! the exports the bytes cross through (`wire::abi`).
use super::*;

/// What a view's store holds: its limits, and the message its panic hook
/// handed over before the trap that follows.
pub(super) struct HostState {
    pub(super) limits: StoreLimits,
    pub(super) panic: Option<String>,
}

/// The guest's exports and the memory their bytes cross in (`wire::abi`).
pub(super) struct Exports {
    /// read directly by the guest tests that poke a view's memory
    pub(super) memory: Memory,
    alloc: TypedFunc<u32, u32>,
    init: TypedFunc<(), ()>,
    tick: TypedFunc<(u32, u32), u64>,
    snapshot: TypedFunc<(), u64>,
    restore: TypedFunc<(u32, u32), u64>,
}

impl Exports {
    pub(super) fn bind(
        store: &mut Store<HostState>,
        instance: &wasmtime::Instance,
    ) -> wasmtime::Result<Self> {
        Ok(Self {
            memory: instance
                .get_memory(&mut *store, "memory")
                .ok_or_else(|| wasmtime::Error::msg("the view exports no memory"))?,
            alloc: instance.get_typed_func(&mut *store, "alloc")?,
            init: instance.get_typed_func(&mut *store, "init")?,
            tick: instance.get_typed_func(&mut *store, "tick")?,
            snapshot: instance.get_typed_func(&mut *store, "snapshot")?,
            restore: instance.get_typed_func(&mut *store, "restore")?,
        })
    }

    /// `bytes` in a buffer the guest allocated and owns from the next call on.
    fn give(&self, store: &mut Store<HostState>, bytes: &[u8]) -> wasmtime::Result<(u32, u32)> {
        let len = u32::try_from(bytes.len())?;
        let ptr = self.alloc.call(&mut *store, len)?;
        self.memory.write(&mut *store, ptr as usize, bytes)?;
        Ok((ptr, len))
    }

    /// The bytes an answer names, copied out before the guest is entered
    /// again; nothing about the pair is trusted.
    fn answer(&self, store: &Store<HostState>, packed: u64) -> wasmtime::Result<Vec<u8>> {
        let (ptr, len) = wire::abi::unpack(packed);
        let start = ptr as usize;
        self.memory
            .data(store)
            .get(start..start.saturating_add(len as usize))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| wasmtime::Error::msg("the view answered outside its memory"))
    }

    fn result(
        &self,
        store: &Store<HostState>,
        packed: u64,
    ) -> wasmtime::Result<Result<Vec<u8>, String>> {
        wire::abi::decode_result(&self.answer(store, packed)?)
            .ok_or_else(|| wasmtime::Error::msg("the view's answer is not a result"))
    }

    pub(super) fn init(&self, store: &mut Store<HostState>) -> wasmtime::Result<()> {
        self.init.call(store, ())
    }

    pub(super) fn tick(
        &self,
        store: &mut Store<HostState>,
        events: &[u8],
    ) -> wasmtime::Result<Vec<u8>> {
        let (ptr, len) = self.give(store, events)?;
        let packed = self.tick.call(&mut *store, (ptr, len))?;
        if wire::abi::unpack(packed).1 as usize > MAX_FRAME_BYTES {
            return Err(wasmtime::Error::msg("frame too large"));
        }
        self.answer(store, packed)
    }

    /// The view's state, held to `MAX_SNAPSHOT_BYTES` on the length the
    /// guest names, before the host copies a byte of it.
    pub(super) fn snapshot(&self, store: &mut Store<HostState>) -> wasmtime::Result<Snapshot> {
        let packed = self.snapshot.call(&mut *store, ())?;
        // the answer's first byte is its result tag
        if wire::abi::unpack(packed).1 as usize > wire::MAX_SNAPSHOT_BYTES + 1 {
            return Ok(Snapshot::TooLarge);
        }
        Ok(match self.result(store, packed)? {
            Ok(state) => Snapshot::Taken(state),
            Err(refusal) => Snapshot::Refused(refusal),
        })
    }

    pub(super) fn restore(
        &self,
        store: &mut Store<HostState>,
        state: &[u8],
    ) -> wasmtime::Result<Result<(), String>> {
        let (ptr, len) = self.give(store, state)?;
        let packed = self.restore.call(&mut *store, (ptr, len))?;
        Ok(self.result(store, packed)?.map(|_| ()))
    }
}

pub(super) fn panic_message(store: &mut Store<HostState>) -> Option<String> {
    let text = store.data_mut().panic.take()?;
    (!text.is_empty()).then_some(text)
}

/// Why a call failed: the trap itself, not the wrapper and backtrace
/// wasmtime prints around it.
pub(super) fn first_line(error: &wasmtime::Error) -> String {
    error
        .root_cause()
        .to_string()
        .lines()
        .next()
        .unwrap_or("trap")
        .to_string()
}
