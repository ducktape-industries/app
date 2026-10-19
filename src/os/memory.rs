//! The process's memory as this OS counts it: macOS's footprint, its
//! graphics ledger and its VM regions by tag; Linux's resident pages, the
//! allocator's bytes in use and `/proc/self/smaps` by mapping.

use ducktape_host::memory::{Figure, OsMemory, Region};
use std::collections::BTreeMap;

fn regions_of(kinds: BTreeMap<String, (u64, u64)>) -> Vec<Region> {
    let mut regions: Vec<Region> = kinds
        .into_iter()
        .map(|(kind, (resident, dirty))| Region {
            kind,
            resident,
            dirty,
        })
        .collect();
    regions.sort_by(|a, b| b.dirty.cmp(&a.dirty).then(b.resident.cmp(&a.resident)));
    regions
}

#[cfg(target_os = "macos")]
pub(super) fn read(regions: bool) -> Option<OsMemory> {
    use mach2::task_info::{TASK_VM_INFO, task_vm_info};
    use mach2::vm_types::natural_t;
    let mut info = task_vm_info::default();
    let mut count = (size_of::<task_vm_info>() / size_of::<natural_t>()) as u32;
    let answered = unsafe {
        mach2::task::task_info(
            mach2::traps::mach_task_self(),
            TASK_VM_INFO,
            (&raw mut info).cast(),
            &mut count,
        )
    };
    if answered != mach2::kern_return::KERN_SUCCESS {
        return None;
    }
    let filled = |end: usize| end <= count as usize * size_of::<natural_t>();
    let peak = std::mem::offset_of!(task_vm_info, ledger_phys_footprint_peak) + size_of::<i64>();
    let graphics = std::mem::offset_of!(task_vm_info, ledger_tag_graphics_footprint_compressed)
        + size_of::<i64>();
    let mut stats = libc::malloc_statistics_t {
        blocks_in_use: 0,
        size_in_use: 0,
        max_size_in_use: 0,
        size_allocated: 0,
    };
    unsafe { libc::malloc_zone_statistics(std::ptr::null_mut(), &mut stats) };
    let footprint_peak = info.ledger_phys_footprint_peak;
    let graphics_bytes =
        info.ledger_tag_graphics_footprint + info.ledger_tag_graphics_footprint_compressed;
    Some(OsMemory {
        figure: Figure::Footprint,
        bytes: info.phys_footprint,
        peak: filled(peak)
            .then(|| u64::try_from(footprint_peak).ok())
            .flatten(),
        heap: Some(stats.size_in_use as u64),
        graphics: filled(graphics)
            .then(|| u64::try_from(graphics_bytes).ok())
            .flatten(),
        regions: regions.then(tagged),
    })
}

#[cfg(target_os = "macos")]
fn tagged() -> Vec<Region> {
    use mach2::vm_region::vm_region_submap_info_64;
    use mach2::vm_types::natural_t;
    let task = unsafe { mach2::traps::mach_task_self() };
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(0);
    let mut kinds: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut address: u64 = 0;
    let mut depth: natural_t = 0;
    loop {
        let mut size: u64 = 0;
        let mut info = vm_region_submap_info_64::default();
        let mut count = vm_region_submap_info_64::count();
        let answered = unsafe {
            mach2::vm::mach_vm_region_recurse(
                task,
                &mut address,
                &mut size,
                &mut depth,
                (&raw mut info).cast(),
                &mut count,
            )
        };
        if answered != mach2::kern_return::KERN_SUCCESS {
            break;
        }
        if info.is_submap != 0 {
            depth += 1;
            continue;
        }
        let (tag, external) = (info.user_tag, info.external_pager != 0);
        let resident = u64::from(info.pages_resident) * page;
        let dirty = (u64::from(info.pages_dirtied) + u64::from(info.pages_swapped_out)) * page;
        let held = kinds.entry(tag_kind(tag, external)).or_default();
        held.0 += resident;
        held.1 += dirty;
        address = address.saturating_add(size);
    }
    regions_of(kinds)
}

#[cfg(target_os = "macos")]
fn tag_kind(tag: u32, external: bool) -> String {
    use mach2::vm_statistics::*;
    let kind = match tag {
        0 if external => "mapped files",
        0 => "untagged anonymous (wasm memory, mmap)",
        VM_MEMORY_MALLOC
        | VM_MEMORY_MALLOC_SMALL
        | VM_MEMORY_MALLOC_LARGE
        | VM_MEMORY_MALLOC_HUGE
        | VM_MEMORY_REALLOC
        | VM_MEMORY_MALLOC_TINY
        | VM_MEMORY_MALLOC_LARGE_REUSABLE
        | VM_MEMORY_MALLOC_LARGE_REUSED
        | VM_MEMORY_MALLOC_NANO
        | VM_MEMORY_MALLOC_MEDIUM
        | VM_MEMORY_MALLOC_PROB_GUARD => "heap (malloc)",
        VM_MEMORY_IOACCELERATOR | VM_MEMORY_IOSURFACE => "GPU (OS regions)",
        VM_MEMORY_COREGRAPHICS
        | VM_MEMORY_COREGRAPHICS_DATA
        | VM_MEMORY_COREGRAPHICS_SHARED
        | VM_MEMORY_COREGRAPHICS_FRAMEBUFFERS
        | VM_MEMORY_COREGRAPHICS_BACKINGSTORES
        | VM_MEMORY_COREGRAPHICS_XALLOC
        | VM_MEMORY_CGIMAGE => "CoreGraphics",
        VM_MEMORY_LAYERKIT => "Core Animation",
        VM_MEMORY_STACK | VM_MEMORY_GUARD => "thread stacks",
        VM_MEMORY_DYLIB | VM_MEMORY_DYLD | VM_MEMORY_DYLD_MALLOC => "libraries (dyld)",
        VM_MEMORY_IOKIT => "IOKit",
        VM_MEMORY_FOUNDATION => "Foundation",
        VM_MEMORY_APPKIT => "AppKit",
        VM_MEMORY_SHARED_PMAP | VM_MEMORY_UNSHARED_PMAP => "page tables (pmap)",
        VM_MEMORY_IMAGEIO => "ImageIO",
        VM_MEMORY_OS_ALLOC_ONCE => "os_alloc_once",
        VM_MEMORY_LIBDISPATCH => "libdispatch",
        VM_MEMORY_ACCELERATE => "Accelerate",
        VM_MEMORY_COREUI | VM_MEMORY_COREUI_CACHED_IMAGE_DATA => "CoreUI images",
        VM_MEMORY_GENEALOGY => "activity tracing (genealogy)",
        VM_MEMORY_COLORSYNC => "ColorSync",
        VM_MEMORY_OBJC_DISPATCHERS => "ObjC dispatchers",
        _ => return format!("tag {tag}"),
    };
    kind.to_owned()
}

#[cfg(target_os = "linux")]
pub(super) fn read(regions: bool) -> Option<OsMemory> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()?;
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let peak = (unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } == 0)
        .then(|| u64::try_from(usage.ru_maxrss).ok())
        .flatten()
        .map(|kib| kib * 1024);
    Some(OsMemory {
        figure: Figure::Resident,
        bytes: pages * page,
        peak,
        heap: heap(),
        graphics: None,
        regions: regions.then(mapped),
    })
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn heap() -> Option<u64> {
    let info = unsafe { libc::mallinfo2() };
    Some((info.uordblks + info.hblkhd) as u64)
}

#[cfg(all(target_os = "linux", not(target_env = "gnu")))]
fn heap() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn mapped() -> Vec<Region> {
    let Ok(smaps) = std::fs::read_to_string("/proc/self/smaps") else {
        return Vec::new();
    };
    let exe = std::env::current_exe()
        .ok()
        .map(|exe| exe.display().to_string());
    regions_of(by_mapping(&smaps, exe.as_deref()))
}

#[cfg(target_os = "linux")]
fn by_mapping(smaps: &str, exe: Option<&str>) -> BTreeMap<String, (u64, u64)> {
    let mut kinds: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut kind = mapping_kind("", exe);
    for line in smaps.lines() {
        let mut words = line.split_whitespace();
        let Some(first) = words.next() else {
            continue;
        };
        if !first.ends_with(':') {
            let path = line
                .split_whitespace()
                .skip(5)
                .collect::<Vec<_>>()
                .join(" ");
            kind = mapping_kind(&path, exe);
            continue;
        }
        let bytes = words
            .next()
            .and_then(|kib| kib.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024;
        match first {
            "Rss:" => kinds.entry(kind.to_owned()).or_default().0 += bytes,
            "Private_Dirty:" | "Swap:" => kinds.entry(kind.to_owned()).or_default().1 += bytes,
            _ => {}
        }
    }
    kinds
}

#[cfg(target_os = "linux")]
fn mapping_kind(path: &str, exe: Option<&str>) -> &'static str {
    let path = path.trim_end_matches(" (deleted)");
    match path {
        "" => "anonymous (malloc arenas, wasm memory, mmap)",
        "[heap]" => "heap (brk)",
        "[vdso]" | "[vvar]" | "[vsyscall]" => "kernel pages",
        _ if path.starts_with("[stack") => "main stack",
        _ if path.starts_with("/dev/dri/")
            || path.starts_with("/dev/nvidia")
            || path.starts_with("anon_inode:")
            || path.contains("dmabuf") =>
        {
            "GPU (OS regions)"
        }
        _ if path.starts_with("/memfd:") => "shared memory (memfd)",
        _ if Some(path) == exe => "this binary",
        _ if path.ends_with(".cwasm") => "compiled views",
        _ if path.ends_with(".so") || path.contains(".so.") => "libraries",
        _ if [".ttf", ".otf", ".ttc"]
            .iter()
            .any(|face| path.ends_with(face)) =>
        {
            "font files"
        }
        _ => "other files",
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn read(_regions: bool) -> Option<OsMemory> {
    None
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn smaps_sum_by_the_kind_of_mapping() {
        let smaps = "\
55d0-55e0 r-xp 00000000 08:01 12 /opt/ducktape/ducktape-app
Rss:                 400 kB
Private_Dirty:         0 kB
VmFlags: rd ex
7f00-7f10 rw-p 00000000 00:00 0
Rss:                1024 kB
Private_Dirty:      1000 kB
Swap:                 24 kB
7f10-7f20 rw-s 00000000 00:05 7 /dev/dri/renderD128
Rss:                 256 kB
Private_Dirty:         0 kB
7f20-7f30 r-xp 00000000 08:01 9 /home/me/.cache/ducktape/view-code/ab-12.cwasm (deleted)
Rss:                  64 kB
";
        let kinds = by_mapping(smaps, Some("/opt/ducktape/ducktape-app"));
        assert_eq!(kinds["this binary"], (400 << 10, 0));
        assert_eq!(
            kinds["anonymous (malloc arenas, wasm memory, mmap)"],
            (1024 << 10, 1024 << 10)
        );
        assert_eq!(kinds["GPU (OS regions)"], (256 << 10, 0));
        assert_eq!(kinds["compiled views"], (64 << 10, 0));
        let live = read(true).expect("this process reads");
        assert_eq!(live.figure, Figure::Resident);
        assert!(live.bytes > 0 && live.regions.is_some_and(|regions| !regions.is_empty()));
    }
}
