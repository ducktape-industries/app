//! Performance counters, on with `DUCKTAPE_PERF=1` (or the phone's debug
//! mode, [`switch`]), read at `GET /perf` (docs/perf.md). One registry for the shell, its windows and every view
//! instance, keyed by `(Key, stage)`. Off, every hook is one relaxed atomic
//! load: [`time`] returns `None` before it reads the clock and the writers
//! return before the lock. On, a sample costs a lock, a map lookup and one
//! ring write. `ducktape::perf` is the one tracing target: the per-tick fuel
//! event and one debug event per sample, filtered at the callsite, plus
//! `perf_summary` and `view_perf` at info.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::runtime::WindowKey;

static ON: AtomicBool = AtomicBool::new(false);
static T0: OnceLock<Instant> = OnceLock::new();

const PERCENTILE_SAMPLE_WINDOW: usize = 256;
const SUMMARY_EVERY: Duration = Duration::from_secs(600);
const RSS_EVERY: Duration = Duration::from_secs(1);

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Called first thing in `main`: `t0` for the startup marks, and with
/// `DUCKTAPE_PERF=1` the switch and the sampler thread.
pub fn start() {
    T0.get_or_init(Instant::now);
    let asked = std::env::var("DUCKTAPE_PERF").ok();
    if asked.as_deref().map(str::trim) != Some("1") {
        return;
    }
    switch(true);
    let _ = std::thread::Builder::new().name("perf".into()).spawn(|| {
        let mut next_summary = Instant::now() + SUMMARY_EVERY;
        loop {
            std::thread::sleep(RSS_EVERY);
            sample_rss();
            if Instant::now() >= next_summary {
                next_summary += SUMMARY_EVERY;
                summary();
            }
        }
    });
}

/// Turns the counters on or off while the app runs (the phone's debug
/// mode, `Prefs::debug`). Off, every hook is the one relaxed load again;
/// what was counted stays for the next time on. The RSS sampler thread is
/// `DUCKTAPE_PERF=1`'s alone: a shell that switches the counters on reads
/// RSS itself, when it shows it ([`sample_rss`]).
pub fn switch(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// What a sample belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    /// One seat: the program and the widget instance showing it.
    View {
        program: &'static str,
        instance: u64,
    },
    Window(WindowKey),
    /// Startup, the entities' methods, I/O, RSS.
    Shell,
}

enum Metric {
    Count(u64),
    Gauge { max: u64, last: u64 },
    Hist(Samples),
}

struct Samples {
    n: u64,
    sum: u64,
    max: u64,
    last: u64,
    ring: Box<[u64; PERCENTILE_SAMPLE_WINDOW]>,
}

impl Samples {
    fn new() -> Self {
        Self {
            n: 0,
            sum: 0,
            max: 0,
            last: 0,
            ring: Box::new([0; PERCENTILE_SAMPLE_WINDOW]),
        }
    }

    fn push(&mut self, value: u64) {
        self.ring[(self.n as usize) % PERCENTILE_SAMPLE_WINDOW] = value;
        self.n += 1;
        self.sum += value;
        self.max = self.max.max(value);
        self.last = value;
    }

    fn recent(&self) -> &[u64] {
        &self.ring[..(self.n as usize).min(PERCENTILE_SAMPLE_WINDOW)]
    }
}

struct Registry {
    metrics: BTreeMap<(Key, &'static str), Metric>,
    marks: BTreeMap<&'static str, u64>,
    since: Instant,
    changed: bool,
}

fn registry() -> MutexGuard<'static, Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            Mutex::new(Registry {
                metrics: BTreeMap::new(),
                marks: BTreeMap::new(),
                since: Instant::now(),
                changed: false,
            })
        })
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A running measurement; its drop records the elapsed µs.
pub struct Timer {
    key: Key,
    stage: &'static str,
    started: Instant,
}

impl Timer {
    pub fn started(&self) -> Instant {
        self.started
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let us = self.started.elapsed().as_micros() as u64;
        write(self.key, self.stage, us, true, |metric| match metric {
            Metric::Hist(samples) => samples.push(us),
            other => *other = fresh_hist(us),
        });
    }
}

fn fresh_hist(value: u64) -> Metric {
    let mut samples = Samples::new();
    samples.push(value);
    Metric::Hist(samples)
}

/// `None` when off: the caller never takes an `Instant`.
pub fn time(key: Key, stage: &'static str) -> Option<Timer> {
    on().then(|| Timer {
        key,
        stage,
        started: Instant::now(),
    })
}

pub fn count(key: Key, stage: &'static str, n: u64) {
    write(key, stage, n, true, |metric| match metric {
        Metric::Count(total) => *total += n,
        other => *other = Metric::Count(n),
    });
}

/// Keeps the largest and the latest value.
pub fn gauge(key: Key, stage: &'static str, value: u64) {
    gauge_flagging(key, stage, value, true);
}

fn gauge_flagging(key: Key, stage: &'static str, value: u64, flag: bool) {
    write(key, stage, value, flag, |metric| match metric {
        Metric::Gauge { max, last } => {
            *max = (*max).max(value);
            *last = value;
        }
        other => {
            *other = Metric::Gauge {
                max: value,
                last: value,
            }
        }
    });
}

/// A sample that is not a time: fuel, bytes, a count per tick.
pub fn record(key: Key, stage: &'static str, value: u64) {
    write(key, stage, value, true, |metric| match metric {
        Metric::Hist(samples) => samples.push(value),
        other => *other = fresh_hist(value),
    });
}

fn write(key: Key, stage: &'static str, value: u64, flag: bool, apply: impl FnOnce(&mut Metric)) {
    if !on() {
        return;
    }
    tracing::debug!(target: "ducktape::perf", ?key, stage, value);
    let mut registry = registry();
    let slot = registry
        .metrics
        .entry((key, stage))
        .or_insert(Metric::Count(0));
    apply(slot);
    registry.changed |= flag;
}

/// A startup milestone: ms since `t0`, kept the first time it is reached.
pub fn mark(stage: &'static str) {
    if !on() {
        return;
    }
    let ms = T0.get().map_or(0, |t0| t0.elapsed().as_millis() as u64);
    tracing::debug!(target: "ducktape::perf", stage, ms, "mark");
    let mut registry = registry();
    registry.marks.entry(stage).or_insert(ms);
    registry.changed = true;
}

/// A milestone reached at `at` rather than now: gpui's first present.
#[cfg(feature = "perf-deep")]
pub fn mark_at(stage: &'static str, at: Instant) {
    if !on() {
        return;
    }
    let ms = T0
        .get()
        .map_or(0, |t0| at.saturating_duration_since(*t0).as_millis() as u64);
    let mut registry = registry();
    registry.marks.entry(stage).or_insert(ms);
    registry.changed = true;
}

/// `stage` with `suffix` appended, interned; built only when on, so an
/// off run never allocates for it.
pub fn suffixed(stage: &str, suffix: &str) -> Option<&'static str> {
    on().then(|| crate::runtime::intern(&format!("{stage}{suffix}")))
}

/// The registry as JSON. Views aggregate by program unless `by_instance`;
/// windows are keyed by their `WindowKey` number (the door adds the name).
pub fn snapshot(by_instance: bool) -> serde_json::Value {
    use serde_json::{Map, Value, json};
    let registry = registry();
    let mut views: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut windows: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut shell = Map::new();
    let mut merged: BTreeMap<(String, &'static str), Vec<&Metric>> = BTreeMap::new();
    for ((key, stage), metric) in &registry.metrics {
        match key {
            Key::View { program, instance } => {
                let name = match by_instance {
                    true => format!("{program}/{instance}"),
                    false => program.to_string(),
                };
                merged.entry((name, stage)).or_default().push(metric);
            }
            Key::Window(window) => {
                windows
                    .entry(window.0.to_string())
                    .or_default()
                    .insert(stage.to_string(), json_of(&[metric]));
            }
            Key::Shell => {
                shell.insert(stage.to_string(), json_of(&[metric]));
            }
        }
    }
    for ((name, stage), metrics) in merged {
        let entry = views.entry(name.clone()).or_default();
        if !by_instance {
            entry.insert("program".into(), json!(name));
        } else if let Some((program, instance)) = name.split_once('/') {
            entry.insert("program".into(), json!(program));
            entry.insert(
                "instance".into(),
                json!(instance.parse::<u64>().unwrap_or(0)),
            );
        }
        entry.insert(stage.to_string(), json_of(&metrics));
    }
    json!({
        "on": on(),
        "since_ms": registry.since.elapsed().as_millis() as u64,
        "startup": registry.marks,
        "views": views,
        "windows": windows,
        "shell": shell,
    })
}

fn json_of(metrics: &[&Metric]) -> serde_json::Value {
    use serde_json::json;
    match metrics[0] {
        Metric::Count(_) => json!(
            metrics
                .iter()
                .map(|metric| match metric {
                    Metric::Count(n) => *n,
                    _ => 0,
                })
                .sum::<u64>()
        ),
        Metric::Gauge { .. } => {
            let (mut top, mut latest) = (0, 0);
            for metric in metrics {
                if let Metric::Gauge { max, last } = metric {
                    top = top.max(*max);
                    latest = latest.max(*last);
                }
            }
            json!({ "max": top, "last": latest })
        }
        Metric::Hist(_) => {
            let (mut n, mut sum, mut max, mut last) = (0u64, 0u64, 0u64, 0u64);
            let mut recent = Vec::new();
            for metric in metrics {
                if let Metric::Hist(samples) = metric {
                    n += samples.n;
                    sum += samples.sum;
                    max = max.max(samples.max);
                    last = samples.last;
                    recent.extend_from_slice(samples.recent());
                }
            }
            recent.sort_unstable();
            let at = |percent: usize| recent.get((recent.len().saturating_sub(1)) * percent / 100);
            json!({
                "n": n,
                "sum": sum,
                "mean": sum.checked_div(n).unwrap_or(0),
                "p50": at(50),
                "p95": at(95),
                "max": max,
                "last": last,
            })
        }
    }
}

/// Clears every counter and sample; the startup marks stay.
#[cfg(any(test, feature = "ax-door", feature = "test-support"))]
pub fn reset() {
    let mut registry = registry();
    registry.metrics.clear();
    registry.since = Instant::now();
    registry.changed = false;
}

/// One `perf_summary` line, if anything changed since the last: on the
/// sampler's cadence, and once when the app quits.
pub fn summary() {
    if !on() || !registry().changed {
        return;
    }
    let snapshot = snapshot(false);
    registry().changed = false;
    tracing::info!(target: "ducktape::perf", snapshot = %snapshot, "perf_summary");
}

/// One `view_perf` line for an instance on its way out: its own numbers,
/// under the same names as `/perf` gives them.
pub fn retire(program: &'static str, instance: u64) {
    if !on() {
        return;
    }
    let all = snapshot(true);
    let name = format!("{program}/{instance}");
    let own = &all["views"][name.as_str()];
    if own.is_null() {
        return;
    }
    tracing::info!(target: "ducktape::perf", program, instance, perf = %own, "view_perf");
}

/// RSS peak and current, in bytes, as `rss.peak` and `rss.current`:
/// `getrusage` gives KiB on Linux and bytes on Apple's systems; the
/// current figure is where `/proc/self/statm` is (Linux, Android), none
/// elsewhere. Nothing while off.
pub fn sample_rss() {
    if !on() {
        return;
    }
    // SAFETY: a plain libc call into a zeroed, stack-local rusage.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return;
        }
        usage
    };
    let unit = crate::contract::os().facts().rss_unit;
    gauge_flagging(Key::Shell, "rss.peak", usage.ru_maxrss as u64 * unit, false);
    if let Some(current) = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|statm| statm.split_whitespace().nth(1)?.parse::<u64>().ok())
    {
        // SAFETY: sysconf reads a constant.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
        gauge_flagging(Key::Shell, "rss.current", current * page, false);
    }
}

#[cfg(any(test, feature = "test-support"))]
pub use test_support::{off_for_test, on_for_test};

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    const VIEW: Key = Key::View {
        program: "perf-test",
        instance: 7,
    };

    #[test]
    fn off_records_nothing_and_reads_no_clock() {
        let _off = off_for_test();
        assert!(time(VIEW, "tick.call").is_none());
        count(VIEW, "renders", 1);
        gauge(VIEW, "nodes", 5);
        record(VIEW, "fuel.tick", 9);
        mark("desk");
        assert_eq!(suffixed("host_call", ".attempts"), None);
        let snapshot = snapshot(true);
        assert_eq!(snapshot["on"], false);
        assert!(snapshot["views"]["perf-test/7"].is_null());
        assert!(snapshot["startup"]["desk"].is_null());
    }

    #[test]
    fn the_ring_is_bounded_and_the_totals_are_not() {
        let _on = on_for_test();
        for value in 1..=300u64 {
            record(VIEW, "frame_bytes", value);
        }
        let stage = &snapshot(true)["views"]["perf-test/7"]["frame_bytes"];
        assert_eq!(stage["n"], 300);
        assert_eq!(stage["max"], 300);
        assert_eq!(stage["last"], 300);
        assert_eq!(stage["p50"], 45 + 255 / 2);
        assert_eq!(stage["p95"], 45 + 255 * 95 / 100);
    }

    #[test]
    fn snapshot_aggregates_by_program_and_reset_clears() {
        let _on = on_for_test();
        let other = Key::View {
            program: "perf-test",
            instance: 8,
        };
        let window = WindowKey::unique();
        let name = window.0.to_string();
        count(VIEW, "renders", 2);
        count(other, "renders", 3);
        gauge(VIEW, "nodes", 10);
        gauge(other, "nodes", 4);
        drop(time(VIEW, "tick.call"));
        count(Key::Window(window), "renders", 1);
        count(Key::Shell, "rail.calls", 1);
        mark("desk");

        let by_program = snapshot(false);
        let view = &by_program["views"]["perf-test"];
        assert_eq!(view["program"], "perf-test");
        assert_eq!(view["renders"], 5);
        assert_eq!(view["nodes"]["max"], 10);
        assert_eq!(view["tick.call"]["n"], 1);
        assert_eq!(by_program["windows"][&name]["renders"], 1);
        assert_eq!(by_program["shell"]["rail.calls"], 1);
        assert!(by_program["startup"]["desk"].is_number());

        let by_instance = snapshot(true);
        assert_eq!(by_instance["views"]["perf-test/7"]["renders"], 2);
        assert_eq!(by_instance["views"]["perf-test/8"]["instance"], 8);

        reset();
        let cleared = snapshot(true);
        assert!(cleared["views"]["perf-test/7"].is_null());
        assert!(cleared["windows"][&name].is_null());
        assert!(cleared["startup"]["desk"].is_number());
    }
}

#[cfg(any(test, feature = "test-support"))]
mod test_support {
    use super::*;

    fn serial() -> MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The switch on and the registry empty for one test, off again after.
    pub struct Switched(#[allow(dead_code)] MutexGuard<'static, ()>);

    impl Drop for Switched {
        fn drop(&mut self) {
            ON.store(false, Ordering::Relaxed);
            reset();
            registry().marks.clear();
        }
    }

    pub fn on_for_test() -> Switched {
        let guard = Switched(serial());
        reset();
        registry().marks.clear();
        ON.store(true, Ordering::Relaxed);
        guard
    }

    pub fn off_for_test() -> Switched {
        let guard = Switched(serial());
        ON.store(false, Ordering::Relaxed);
        reset();
        registry().marks.clear();
        guard
    }
}
