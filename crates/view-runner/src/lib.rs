//! The view runner's contract: what runs a view's code (wasmtime, JavaScriptCore, the page's own WebAssembly) and how its answers land.
//! It names no runtime and no window, so every runner builds on every target; the rules each runner keeps are in README.md.

use std::path::Path;
use std::sync::Arc;

pub trait ViewRunner: Send + Sync + 'static {
    fn prepare(
        &self,
        hash: [u8; 32],
        code: Code<'_>,
        view_bytes: usize,
    ) -> Result<Prepared, Failure>;
    fn instantiate(&self, hash: [u8; 32], seat: SeatId, land: Landing);
    fn describe(
        &self,
        hash: [u8; 32],
        module: Code<'_>,
        op: Vec<u8>,
    ) -> rt::Fut<'static, Option<describe::Description>>;
    fn code_memory(&self) -> Vec<HeldCode> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Code<'a> {
    pub bytes: &'a [u8],
    pub file: Option<(&'a Path, u64)>,
}

impl<'a> Code<'a> {
    pub fn of(bytes: &'a [u8]) -> Self {
        Self { bytes, file: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeSource {
    Memory,
    Disk,
    Cold,
}

impl CodeSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Disk => "disk",
            Self::Cold => "cold",
        }
    }
}

#[derive(Clone)]
pub struct Prepared {
    pub source: CodeSource,
    pub hold: Arc<dyn std::any::Any + Send + Sync>,
}

pub trait ViewInstance: Send {
    fn code_hash(&self) -> [u8; 32];
    fn begin_init(&mut self);
    fn begin_tick(&mut self, events: Vec<u8>);
    fn begin_snapshot(&mut self);
    fn begin_restore(&mut self, snapshot: Vec<u8>);
    fn fuel_used(&self) -> u64;
    fn memory_bytes(&self) -> usize;
    fn memory_resident(&self) -> Option<u64> {
        None
    }
    fn stop(self: Box<Self>);
    #[cfg(feature = "test-support")]
    fn memory_for_test(&self) -> &[u8] {
        unimplemented!("only the wasmtime instance lends its memory to a test")
    }
    #[cfg(feature = "test-support")]
    fn memory_mut_for_test(&mut self) -> &mut [u8] {
        unimplemented!("only the wasmtime instance lends its memory to a test")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeatId(pub &'static str, pub u64);

#[derive(Clone, Debug)]
pub struct Landing(async_channel::Sender<Landed>);

impl Landing {
    pub fn channel() -> (Self, async_channel::Receiver<Landed>) {
        let (sender, receiver) = async_channel::unbounded();
        (Self(sender), receiver)
    }

    pub fn send(&self, landed: Landed) {
        let _ = self.0.try_send(landed);
    }
}

pub enum Landed {
    Seated(Box<dyn ViewInstance>),
    Inited,
    Frame(Vec<u8>),
    Snapshot(Vec<u8>),
    SnapshotRefused(String),
    Restored,
    RestoreRefused(String),
    Fault(Fault),
}

impl std::fmt::Debug for Landed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Seated(instance) => write!(f, "Seated({:02x?})", &instance.code_hash()[..4]),
            Self::Inited => f.write_str("Inited"),
            Self::Frame(bytes) => write!(f, "Frame({} bytes)", bytes.len()),
            Self::Snapshot(bytes) => write!(f, "Snapshot({} bytes)", bytes.len()),
            Self::SnapshotRefused(why) => write!(f, "SnapshotRefused({why})"),
            Self::Restored => f.write_str("Restored"),
            Self::RestoreRefused(why) => write!(f, "RestoreRefused({why})"),
            Self::Fault(fault) => write!(f, "Fault({fault})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    Trap(String),
    Panicked(String),
    Memory,
    Runner(String),
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Trap(words) | Self::Panicked(words) | Self::Runner(words) => f.write_str(words),
            Self::Memory => f.write_str("the view is past the device's view memory setting"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Unsupported(String),
    Runner(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(words) | Self::Runner(words) => f.write_str(words),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldCode {
    pub hash: [u8; 32],
    pub mapped: u64,
    pub resident: Option<u64>,
    pub describe: bool,
}
