//! A call's codecs: STUB. The codec lane (`feat/huddle-codec`) brings the
//! real ones (Opus by libopus as Rust, H.264 by OpenH264) under these
//! exact names; until that branch lands, every constructor says this
//! build has no codec, so a publishing piece reports `Unsupported` and
//! nothing else changes. Delete this file on the rebase.
use std::time::Duration;

use crate::Picture;

/// Samples a second, in and out.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples in one packet: 20 ms.
pub const FRAME: usize = 960;

const NO_CODEC: &str = "this build has no codec";

/// A person's voice into packets, one per 20 ms.
pub struct AudioEncoder(());

impl AudioEncoder {
    pub fn new(_bitrate: u32) -> Result<Self, String> {
        Err(NO_CODEC.into())
    }

    pub fn set_bitrate(&mut self, _bitrate: u32) -> Result<(), String> {
        Err(NO_CODEC.into())
    }

    pub fn encode(&mut self, _pcm: &[f32; FRAME]) -> Result<Vec<u8>, String> {
        Err(NO_CODEC.into())
    }
}

/// Packets back into a voice; a lost packet concealed.
pub struct AudioDecoder(());

impl AudioDecoder {
    pub fn new() -> Result<Self, String> {
        Err(NO_CODEC.into())
    }

    pub fn decode(&mut self, _packet: &[u8]) -> Result<[f32; FRAME], String> {
        Err(NO_CODEC.into())
    }

    /// The packet that did not come, rebuilt from `next` (the one after
    /// it, FEC) or concealed from what came before.
    pub fn lost(&mut self, _next: Option<&[u8]>) -> Result<[f32; FRAME], String> {
        Err(NO_CODEC.into())
    }
}

/// One encoded picture.
#[derive(Clone, Debug)]
pub struct VideoPacket {
    pub data: Vec<u8>,
    /// A decoder can start here.
    pub keyframe: bool,
}

/// A camera's pictures into packets.
pub struct VideoEncoder(());

impl VideoEncoder {
    pub fn new(_width: u32, _height: u32, _bitrate: u32) -> Result<Self, String> {
        Err(NO_CODEC.into())
    }

    pub fn set_bitrate(&mut self, _bitrate: u32) -> Result<(), String> {
        Err(NO_CODEC.into())
    }

    pub fn set_size(&mut self, _width: u32, _height: u32) -> Result<(), String> {
        Err(NO_CODEC.into())
    }

    pub fn keyframe(&mut self) {}

    pub fn encode(
        &mut self,
        _picture: &Picture,
        _at: Duration,
    ) -> Result<Option<VideoPacket>, String> {
        Err(NO_CODEC.into())
    }
}

/// Packets back into pictures.
pub struct VideoDecoder(());

impl VideoDecoder {
    pub fn new() -> Result<Self, String> {
        Err(NO_CODEC.into())
    }

    pub fn decode(&mut self, _packet: &[u8]) -> Result<Option<Picture>, String> {
        Err(NO_CODEC.into())
    }
}
