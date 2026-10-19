//! The windows each network last had, as `windows.json` beside the prefs keeps them: every window's
//! bounds and full size, its panes, and the route each pane's view showed. Layout only, no view's
//! state; `Windows` keeps it as the windows move and takes it up once a network's desk first shows.
use crate::ui::layout::{Frame, Layout, Pane};
use gpui_kit::{App, Bounds, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Arrangement {
    pub(crate) windows: Vec<KeptWindow>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct KeptWindow {
    pub(crate) console: bool,
    pub(crate) state: State,
    pub(crate) frame: Frame,
    pub(crate) desk: KeptDesk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Windowed,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct KeptDesk {
    pub(crate) panes: Vec<KeptPane>,
    pub(crate) focused: usize,
    pub(crate) filled: bool,
    pub(crate) desk: Option<(f32, f32)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct KeptPane {
    pub(crate) program: String,
    pub(crate) frame: Option<Frame>,
    pub(crate) z: u64,
    pub(crate) by_hand: bool,
    pub(crate) showing: Option<String>,
}

pub(crate) struct Open<'a> {
    pub(crate) console: bool,
    pub(crate) floated: bool,
    pub(crate) bounds: WindowBounds,
    pub(crate) layout: &'a Layout,
}

impl Arrangement {
    pub(crate) fn of<'a>(open: impl IntoIterator<Item = Open<'a>>) -> Self {
        let mut windows = Vec::new();
        let mut floated = Vec::new();
        for open in open {
            let desk = KeptDesk::of(open.layout);
            match (open.console, open.floated) {
                (_, true) => floated.extend(desk.panes),
                (false, false) if desk.panes.len() != 1 => {}
                (console, false) => {
                    let (state, frame) = kept_bounds(open.bounds);
                    windows.push(KeptWindow {
                        console,
                        state,
                        frame,
                        desk,
                    });
                }
            }
        }
        if let Some(console) = windows.iter_mut().find(|window| window.console) {
            for mut pane in floated {
                let top = console.desk.panes.iter().map(|pane| pane.z).max();
                pane.z = top.unwrap_or(0) + 1;
                pane.frame = None;
                pane.by_hand = false;
                console.desk.panes.push(pane);
            }
        }
        Self { windows }
    }
}

impl KeptWindow {
    pub(crate) fn bounds(&self) -> WindowBounds {
        let Frame { x, y, w, h } = self.frame;
        let at = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
        match self.state {
            State::Windowed => WindowBounds::Windowed(at),
            State::Maximized => WindowBounds::Maximized(at),
            State::Fullscreen => WindowBounds::Fullscreen(at),
        }
    }
}

fn kept_bounds(bounds: WindowBounds) -> (State, Frame) {
    let (state, at) = match bounds {
        WindowBounds::Windowed(at) => (State::Windowed, at),
        WindowBounds::Maximized(at) => (State::Maximized, at),
        WindowBounds::Fullscreen(at) => (State::Fullscreen, at),
    };
    let frame = Frame {
        x: f32::from(at.origin.x),
        y: f32::from(at.origin.y),
        w: f32::from(at.size.width),
        h: f32::from(at.size.height),
    };
    (state, frame)
}

pub(crate) fn on_a_display(at: Bounds<Pixels>, displays: &[Bounds<Pixels>]) -> bool {
    displays.is_empty() || displays.iter().any(|display| display.intersects(&at))
}

impl KeptDesk {
    fn of(layout: &Layout) -> Self {
        Self {
            panes: layout.panes.iter().map(KeptPane::of).collect(),
            focused: layout.focused,
            filled: layout.filled,
            desk: layout.desk,
        }
    }

    pub(crate) fn layout(&self) -> Layout {
        let panes = self.panes.iter().map(KeptPane::pane).collect();
        let desk = self.desk.filter(|(w, h)| w.is_finite() && h.is_finite());
        Layout::restored(panes, self.focused, self.filled, desk)
    }
}

impl KeptPane {
    fn of(pane: &Pane) -> Self {
        Self {
            program: pane.program.to_owned(),
            frame: pane.frame,
            z: pane.z,
            by_hand: pane.by_hand,
            showing: pane.showing.clone(),
        }
    }

    fn pane(&self) -> Pane {
        let mut pane = Pane::new(crate::runtime::intern(&self.program));
        let finite = |frame: &Frame| {
            [frame.x, frame.y, frame.w, frame.h]
                .iter()
                .all(|v| v.is_finite())
        };
        pane.frame = self.frame.filter(finite);
        pane.z = self.z;
        pane.by_hand = self.by_hand;
        pane.showing = self.showing.clone().filter(|showing| !showing.is_empty());
        pane
    }
}

pub(crate) struct Keeper {
    path: Option<PathBuf>,
    networks: BTreeMap<String, Arrangement>,
    last: Option<(u64, Vec<u8>)>,
    queued: Arc<Mutex<Queued>>,
    written: Arc<Mutex<u64>>,
}

#[derive(Default)]
struct Queued {
    seq: u64,
    bytes: Option<Vec<u8>>,
    writing: bool,
}

impl Keeper {
    pub(crate) fn load() -> Self {
        let path = crate::backend::dirs().config.join("windows.json");
        let read = crate::backend::read_or_set_aside(&path, |bytes| {
            serde_json::from_slice::<BTreeMap<String, Arrangement>>(bytes)
        });
        let (path, networks) = match read {
            Ok(networks) => (Some(path), networks.unwrap_or_default()),
            Err(error) => {
                tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "the windows' arrangement is unreadable and is not kept");
                (None, BTreeMap::new())
            }
        };
        Self {
            path,
            networks,
            last: None,
            queued: Arc::default(),
            written: Arc::default(),
        }
    }

    pub(crate) fn of(&self, chain: &str) -> Option<&Arrangement> {
        self.networks.get(chain)
    }

    pub(crate) fn keep(&mut self, chain: &str, arrangement: Arrangement, cx: &App) {
        if self.networks.get(chain) == Some(&arrangement) {
            return;
        }
        self.networks.insert(chain.to_owned(), arrangement);
        let Some(path) = self.path.clone() else {
            return;
        };
        let bytes = match serde_json::to_vec_pretty(&self.networks) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(target: "ducktape::app", %error, "the windows' arrangement was not kept");
                return;
            }
        };
        let mut queued = lock(&self.queued);
        queued.seq += 1;
        queued.bytes = Some(bytes.clone());
        self.last = Some((queued.seq, bytes));
        if std::mem::replace(&mut queued.writing, true) {
            return;
        }
        drop(queued);
        let (queued, written) = (self.queued.clone(), self.written.clone());
        cx.background_executor()
            .spawn(async move {
                while let Some((seq, bytes)) = next(&queued) {
                    write(&path, &written, seq, &bytes);
                }
            })
            .detach();
    }

    pub(crate) fn flush(&self) {
        if let (Some(path), Some((seq, bytes))) = (&self.path, &self.last) {
            write(path, &self.written, *seq, bytes);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn next(queued: &Mutex<Queued>) -> Option<(u64, Vec<u8>)> {
    let mut queued = lock(queued);
    let seq = queued.seq;
    let taken = queued.bytes.take().map(|bytes| (seq, bytes));
    queued.writing = taken.is_some();
    taken
}

fn write(path: &Path, written: &Mutex<u64>, seq: u64, bytes: &[u8]) {
    let mut at = lock(written);
    if seq <= *at {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match noded_client::fs::atomic_write(path, bytes, false) {
        Ok(()) => *at = seq,
        Err(error) => {
            tracing::warn!(target: "ducktape::app", path = %path.display(), %error, "the windows' arrangement was not kept")
        }
    }
}
