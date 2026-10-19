//! The app's memory by owner: what the OS counts for the process and its
//! regions, beside the bytes each owner the host holds.

use serde::Serialize;
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Reading {
    pub os: Option<OsMemory>,
    pub owners: Vec<Owned>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OsMemory {
    pub figure: Figure,
    pub bytes: u64,
    pub peak: Option<u64>,
    pub heap: Option<u64>,
    pub graphics: Option<u64>,
    pub regions: Option<Vec<Region>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Figure {
    Footprint,
    Resident,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Region {
    pub kind: String,
    pub resident: u64,
    pub dirty: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Owned {
    pub group: Group,
    pub name: String,
    pub bytes: u64,
    pub resident: Option<u64>,
    pub clean: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    View,
    Pictures,
    Notifier,
    Code,
    Rasters,
    SvgRasters,
    Fonts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldCode {
    pub hash: [u8; 32],
    pub mapped: u64,
    pub resident: Option<u64>,
    pub describe: bool,
}

pub fn read(regions: bool) -> Reading {
    #[cfg_attr(not(feature = "ui"), allow(unused_mut))]
    let mut owners = crate::runtime::memory_owners();
    #[cfg(feature = "ui")]
    owners.push(faces());
    Reading {
        os: crate::contract::os().memory(regions),
        owners,
    }
}

#[cfg(feature = "ui")]
pub type Drawn = (
    String,
    gpui_kit::AnyWindowHandle,
    gpui_kit::Entity<crate::render::ViewTree>,
);

#[cfg(feature = "ui")]
pub fn drawn(
    reading: &mut Reading,
    seated: &[Drawn],
    windows: &[(String, gpui_kit::AnyWindowHandle)],
    cx: &gpui_kit::App,
) {
    for (name, _, tree) in seated {
        reading.owners.push(Owned {
            group: Group::Rasters,
            name: name.clone(),
            bytes: tree.read(cx).raster_bytes(),
            resident: None,
            clean: false,
        });
    }
    for (name, window) in windows {
        reading.owners.push(Owned {
            group: Group::SvgRasters,
            name: name.clone(),
            bytes: crate::render::svg_raster_bytes(window.window_id(), cx),
            resident: None,
            clean: false,
        });
    }
}

#[cfg(feature = "ui")]
fn faces() -> Owned {
    let mut faces = Owned {
        group: Group::Fonts,
        name: "bundled faces".to_owned(),
        bytes: 0,
        resident: Some(0),
        clean: true,
    };
    for face in crate::fonts::BUNDLED_FACES {
        faces.bytes += face.len() as u64;
        faces.resident = faces
            .resident
            .zip(resident(face.as_ptr(), face.len()))
            .map(|(held, more)| held + more);
    }
    faces
}

pub struct Gauge {
    group: Group,
    name: String,
    code: Option<[u8; 32]>,
    held: Mutex<(u64, Option<u64>)>,
}

impl Gauge {
    pub fn set(&self, bytes: u64, resident: Option<u64>) {
        *lock(&self.held) = (bytes, resident);
    }
}

fn gauges() -> &'static Mutex<Vec<Weak<Gauge>>> {
    static GAUGES: Mutex<Vec<Weak<Gauge>>> = Mutex::new(Vec::new());
    &GAUGES
}

pub fn track(group: Group, name: String, code: Option<[u8; 32]>) -> Arc<Gauge> {
    let gauge = Arc::new(Gauge {
        group,
        name,
        code,
        held: Mutex::new((0, None)),
    });
    let mut gauges = lock(gauges());
    gauges.retain(|held| held.strong_count() > 0);
    gauges.push(Arc::downgrade(&gauge));
    gauge
}

pub(crate) fn tracked() -> Vec<(Owned, Option<[u8; 32]>)> {
    let live: Vec<Arc<Gauge>> = lock(gauges()).iter().filter_map(Weak::upgrade).collect();
    live.iter()
        .map(|gauge| {
            let (bytes, resident) = *lock(&gauge.held);
            let owned = Owned {
                group: gauge.group,
                name: gauge.name.clone(),
                bytes,
                resident,
                clean: false,
            };
            (owned, gauge.code)
        })
        .collect()
}

pub fn resident(start: *const u8, len: usize) -> Option<u64> {
    if len == 0 {
        return Some(0);
    }
    let size = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) })
        .ok()
        .filter(|size| *size > 0)?;
    let first = start as usize / size * size;
    let end = (start as usize).checked_add(len)?.div_ceil(size) * size;
    let mut pages = vec![0u8; (end - first) / size];
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touched_pages_read_resident_and_a_gauge_lives_with_its_holder() {
        let size = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap();
        let touched = vec![1u8; size * 4];
        let counted = resident(touched.as_ptr(), touched.len()).expect("mincore answers");
        assert!(counted >= (size * 4) as u64, "{counted}");
        assert_eq!(resident(touched.as_ptr(), 0), Some(0));

        let gauge = track(Group::Notifier, "gauge-test".to_owned(), Some([3; 32]));
        gauge.set(7, Some(5));
        let held = |name: &str| tracked().into_iter().find(|(owned, _)| owned.name == name);
        let (owned, code) = held("gauge-test").expect("tracked while held");
        assert_eq!(
            (owned.bytes, owned.resident, code),
            (7, Some(5), Some([3; 32]))
        );
        drop(gauge);
        assert!(held("gauge-test").is_none(), "gone with its holder");
    }
}
