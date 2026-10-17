//! Debug mode (App settings): what each open program costs, under its name
//! on the Open tab, and the whole app's memory over the list. Read once a
//! second while the tab is shown in front, never else; the numbers are the
//! host's own (`ducktape_host::perf`, the registry `GET /perf` reads, which
//! the switch turns on) and each view's memory as its runner reports it
//! (`Seat::memory_bytes`). No counter of the phone's own.

use super::Root;
use gpui_kit::{Context, Task, Window};
use serde_json::Value;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How often the numbers are read while shown.
const EVERY: Duration = Duration::from_secs(1);
/// The readings a rate spans: now and up to three seconds back.
const WINDOW: usize = 4;

/// The perf registry as read over the last few seconds, oldest first, and
/// the loop reading it while the Open tab is shown.
#[derive(Default)]
pub struct Readout {
    readings: VecDeque<(Instant, Value)>,
    reading: Option<Task<()>>,
}

impl Readout {
    /// Reads now and once a second while `shown` (debug mode on, the Open
    /// tab on screen); stopped, and its readings dropped, once not. A
    /// second the window is not in front reads nothing.
    pub fn follow(&mut self, shown: bool, window: &mut Window, cx: &mut Context<Root>) {
        match (shown, self.reading.is_some()) {
            (true, false) => {
                self.read(cx.background_executor().now());
                self.reading = Some(cx.spawn_in(window, async move |root, cx| {
                    loop {
                        cx.background_executor().timer(EVERY).await;
                        let read = root.update_in(cx, |root, window, cx| {
                            if window.is_window_active() {
                                root.readout.read(cx.background_executor().now());
                                cx.notify();
                            }
                        });
                        if read.is_err() {
                            break;
                        }
                    }
                }));
            }
            (false, true) => *self = Self::default(),
            _ => {}
        }
    }

    fn read(&mut self, now: Instant) {
        ducktape_host::perf::sample_rss();
        self.readings
            .push_back((now, ducktape_host::perf::snapshot(true)));
        if self.readings.len() > WINDOW {
            self.readings.pop_front();
        }
    }

    /// The app's line: its resident memory now and at its peak.
    pub fn app(&self) -> String {
        let rss = |stage: &str| {
            let (_, last) = self.readings.back()?;
            last["shell"][stage]["last"].as_u64()
        };
        format!(
            "app {} resident · peak {}",
            mib(rss("rss.current")),
            mib(rss("rss.peak"))
        )
    }

    /// What `program`'s seat `instance` costs over the readings, its
    /// view's linear memory `memory` beside.
    pub fn cost(&self, program: &str, instance: u64, memory: Option<usize>) -> Cost {
        let name = format!("{program}/{instance}");
        let at = |reading: &Value, stage: &str, field: &str| {
            reading["views"][name.as_str()][stage][field].as_u64()
        };
        // per second between the first reading and the last; a stage not
        // there yet was never written: nothing since the counters came on
        let rate = |stage: &str, field: &str| {
            let ((then, first), (now, last)) = (self.readings.front()?, self.readings.back()?);
            let seconds = now.saturating_duration_since(*then).as_secs_f64();
            let grew = at(last, stage, field)
                .unwrap_or(0)
                .saturating_sub(at(first, stage, field).unwrap_or(0));
            (seconds > 0.).then(|| grew as f64 / seconds)
        };
        Cost {
            memory,
            busy_us: rate("tick.call", "sum"),
            ticks: rate("tick.call", "n"),
            fuel: self
                .readings
                .back()
                .and_then(|(_, last)| at(last, "fuel.tick", "last")),
        }
    }
}

/// One program's cost, as its two lines say it; `None` is a number not to
/// be had (no view running, a rate before its second reading).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cost {
    /// Its view's linear memory, bytes.
    pub memory: Option<usize>,
    /// The time its ticks took, µs per second.
    pub busy_us: Option<f64>,
    /// Its ticks (each one call into the view, one frame back), per second.
    pub ticks: Option<f64>,
    /// The fuel its last tick burned.
    pub fuel: Option<u64>,
}

impl Cost {
    /// Its lines: memory and time, then ticks and fuel, each with its unit.
    pub fn lines(&self) -> [String; 2] {
        let or = |shown: Option<String>| shown.unwrap_or_else(|| "n/a".to_owned());
        let memory = mib(self.memory.map(|bytes| bytes as u64));
        let busy = or(self.busy_us.map(|us| format!("{:.1}", us / 1000.)));
        let ticks = or(self.ticks.map(|ticks| format!("{ticks:.1}")));
        let fuel = or(self.fuel.map(|fuel| match fuel {
            0..1_000 => fuel.to_string(),
            1_000..1_000_000 => format!("{:.1}k", fuel as f64 / 1e3),
            _ => format!("{:.1}M", fuel as f64 / 1e6),
        }));
        [
            format!("mem {memory} · cpu {busy} ms/s"),
            format!("{ticks} ticks/s · last tick {fuel} fuel"),
        ]
    }
}

/// Bytes in MiB, one decimal; `n/a` for none.
fn mib(bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) => format!("{:.1} MiB", bytes as f64 / (1024. * 1024.)),
        None => "n/a".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A number not to be had reads `n/a`, never 0; each says its unit.
    #[test]
    fn a_number_not_had_reads_n_a_and_each_says_its_unit() {
        let none = Readout::default();
        assert_eq!(none.app(), "app n/a resident · peak n/a");
        assert_eq!(
            none.cost("chat", 1, None).lines(),
            [
                "mem n/a · cpu n/a ms/s".to_owned(),
                "n/a ticks/s · last tick n/a fuel".to_owned()
            ]
        );
        let cost = Cost {
            memory: Some(3 << 20),
            busy_us: Some(1500.),
            ticks: Some(2.),
            fuel: Some(1_300_000),
        };
        assert_eq!(
            cost.lines(),
            [
                "mem 3.0 MiB · cpu 1.5 ms/s".to_owned(),
                "2.0 ticks/s · last tick 1.3M fuel".to_owned()
            ]
        );
    }
}
