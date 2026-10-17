//! Ids unique on this device.

/// One id, unique on this device, for a record a view mints.
pub fn fresh_id(prefix: &str) -> String {
    format!("{prefix}-{:x}-{}", epoch_nanos(), fresh_counter())
}

fn epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default()
}

/// Only ever climbs within this process (1, then the clock's microseconds
/// or the last value plus one, whichever is later); only [`fresh_id`] mixes
/// it in.
pub fn fresh_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = (epoch_nanos() / 1_000) as u64;
    LAST.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| {
        Some(now.max(last + 1))
    })
    .unwrap_or(now)
        + 1
}
