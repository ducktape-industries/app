//! What the process knows of itself where targets differ: the local time's offset, its pages and how many are resident.
//! libc's natively; in a browser the page's `Date`, and none of what a page can't see.

#[cfg(not(target_family = "wasm"))]
pub fn id() -> u32 {
    std::process::id()
}

#[cfg(not(target_family = "wasm"))]
pub fn local_offset(wall: i64) -> i64 {
    let time = wall as libc::time_t;
    // SAFETY: `localtime_r` writes only the `tm` it is handed.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        match libc::localtime_r(&time, &mut tm).is_null() {
            true => 0,
            false => tm.tm_gmtoff as i64,
        }
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn page_size() -> Option<usize> {
    // SAFETY: sysconf reads a constant.
    usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) })
        .ok()
        .filter(|size| *size > 0)
}

#[cfg(not(target_family = "wasm"))]
pub fn resident(start: *const u8, len: usize) -> Option<u64> {
    if len == 0 {
        return Some(0);
    }
    let size = page_size()?;
    let first = start as usize / size * size;
    let end = (start as usize).checked_add(len)?.div_ceil(size) * size;
    let mut pages = vec![0u8; (end - first) / size];
    // SAFETY: mincore writes one byte per page of the range into `pages`, which holds exactly that many.
    let answered = unsafe {
        libc::mincore(
            first as *mut libc::c_void,
            end - first,
            pages.as_mut_ptr().cast(),
        )
    };
    (answered == 0)
        .then(|| pages.iter().filter(|page| **page & 1 != 0).count() as u64 * size as u64)
}

#[cfg(not(target_family = "wasm"))]
pub fn max_rss() -> Option<u64> {
    // SAFETY: a plain libc call into a zeroed, stack-local rusage.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return None;
        }
        usage
    };
    Some(usage.ru_maxrss as u64)
}

#[cfg(not(target_family = "wasm"))]
pub fn resident_pages() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    statm.split_whitespace().nth(1)?.parse::<u64>().ok()
}

#[cfg(target_family = "wasm")]
pub fn id() -> u32 {
    static DRAWN: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *DRAWN.get_or_init(|| (js_sys::Math::random() * f64::from(u32::MAX)) as u32)
}

#[cfg(target_family = "wasm")]
pub fn local_offset(wall: i64) -> i64 {
    let at = wasm_bindgen::JsValue::from_f64(wall as f64 * 1000.0);
    (-js_sys::Date::new(&at).get_timezone_offset() * 60.0) as i64
}

#[cfg(target_family = "wasm")]
pub fn page_size() -> Option<usize> {
    None
}

#[cfg(target_family = "wasm")]
pub fn resident(start: *const u8, len: usize) -> Option<u64> {
    let _ = (start, len);
    None
}

#[cfg(target_family = "wasm")]
pub fn max_rss() -> Option<u64> {
    None
}

#[cfg(target_family = "wasm")]
pub fn resident_pages() -> Option<u64> {
    None
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    #[test]
    fn touched_pages_read_resident() {
        let size = page_size().expect("a page size");
        let touched = vec![1u8; size * 4];
        let counted = resident(touched.as_ptr(), touched.len()).expect("mincore answers");
        assert!(counted >= (size * 4) as u64, "{counted}");
        assert_eq!(resident(touched.as_ptr(), 0), Some(0));
        assert!(max_rss().is_some_and(|peak| peak > 0));
    }

    #[test]
    fn the_process_id_is_the_os_pid() {
        assert_eq!(id(), std::process::id());
    }
}
