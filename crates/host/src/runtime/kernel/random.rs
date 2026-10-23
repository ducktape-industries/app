//! `host.random`: bytes from the system's secure random source.
use rand::RngCore as _;

use super::*;

pub(super) fn random(view: &mut View, id: u64, payload: &[u8]) {
    let count = match methods::decode::<u32>(payload) {
        Ok(count) => count,
        Err(error) => return view.refuse(id, refusal::MALFORMED_REQUEST, error),
    };
    if u64::from(count) > crate::backend::memory_bounds().view_bytes {
        let past = format!("{count} bytes are more than a view's memory holds");
        return view.refuse(id, refusal::MALFORMED_REQUEST, past);
    }
    let mut bytes = vec![0; count as usize];
    match rand::rngs::OsRng.try_fill_bytes(&mut bytes) {
        Ok(()) => view.reply(id, Ok(methods::encode(&bytes))),
        Err(error) => view.reply(id, Err(host_fault(error))),
    }
}
