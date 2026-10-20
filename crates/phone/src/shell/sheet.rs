//! The phone's bottom sheets (Expo `Sheet`, `SheetHead`, `SheetRow`, mobile
//! `src/kit/Sheet.tsx`): docked to the bottom over the scrim, square, a
//! hairline over the top, the 20 handle band, padded by the bottom inset.
//! A press on the scrim closes the sheet, as Back does; a sheet is the
//! first thing Back closes (SPEC §7.6). It slides up by its own height as
//! it comes and down as it goes, the scrim fading with it (gorhom's
//! `BottomSheetModal` and `BottomSheetBackdrop`, `motion::Idiom::sheet`); a
//! sheet going draws what it drew open and takes no press.
//!
//! A finger drags the shell's sheets down (UIKit's sheets, gorhom's pan
//! down to close): the panel follows it, and let go past half its height,
//! its speed counted as a swipe back's is ([`swipe::crosses`]), it goes on
//! down at that speed and closes, else it springs back up. Under Reduce
//! Motion nothing follows the finger; a release that goes closes it, a
//! fade. A sheet with a field sits on the keyboard: the bottom inset it is
//! padded by is the keyboard's while one is up.

use super::look::{self, Palette};
use super::{Root, swipe};
use ducktape_host::a11y::Control as _;
use ducktape_host::motion::{self, Tween};
use gpui_kit::{
    AnyElement, App, Context, Div, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Role, ScrollWheelEvent, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, TouchPhase, canvas, div, px, svg,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A sheet the shell has open over the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// The capsule's `···` over `program`.
    Capsule(&'static str),
    /// The three words for `program`'s notices.
    Notifications(&'static str),
    /// Add a device, from the Account tab: the code the new device shows,
    /// then its key's fingerprint and this phone's yes.
    AddDevice,
    /// Switch network, from the Account tab: another node's address, and
    /// the nodes reached before.
    Switch,
}

/// Where a sheet is drawn: the window's bottom inset and the room over it,
/// how far in it has come, and its panel's height as last laid out.
pub struct At {
    /// The bottom inset the panel is padded by.
    pub bottom: Pixels,
    /// The tallest the panel is.
    pub room: Pixels,
    /// How far in it is: 0 gone, 1 in place.
    pub progress: f32,
    /// Whether it slides (else, under Reduce Motion, it fades in place).
    pub slides: bool,
    /// Whether it is going: it takes no press.
    pub leaving: bool,
    /// The panel's height at its last layout; `None` before the first.
    pub height: Rc<Cell<Option<Pixels>>>,
    /// Whether a finger drags it down to close it ([`Root::sheet_panned`]):
    /// the shell's sheets, not the consent sheet.
    pub drags: bool,
}

/// A sheet over the window, `id` its element's: the scrim over everything,
/// `content` docked to its bottom under the handle, at most `at.room`
/// tall, padded by the bottom inset, as far in as `at` says; a press on the
/// scrim is `close`, and a finger drags it while `at.drags`.
pub fn frame(
    id: &'static str,
    name: &'static str,
    content: impl IntoElement,
    at: At,
    close: fn(&mut Root, &mut Context<Root>),
    palette: Palette,
    cx: &mut Context<Root>,
) -> AnyElement {
    let handle = div()
        .flex_none()
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(32.))
                .h(px(4.))
                .rounded(px(2.))
                .bg(palette.border_strong()),
        );
    let down = match (at.slides, at.height.get()) {
        (false, _) => px(0.),
        (true, Some(height)) => height * (1. - at.progress),
        (true, None) => at.room + at.bottom,
    };
    let height = at.height.clone();
    let measure = canvas(
        move |bounds, _, _| height.set(Some(bounds.size.height)),
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0();
    let mut panel = div()
        .id(id)
        .control(Role::Dialog, name)
        .debug_selector(move || id.to_owned())
        .occlude()
        .relative()
        .top(down)
        .opacity(if at.slides { 1. } else { at.progress })
        .flex()
        .flex_col()
        .w_full()
        .max_h(at.room)
        .pb(at.bottom)
        .bg(palette.background())
        .border_t_1()
        .border_color(palette.border_strong())
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(measure)
        .child(handle)
        .child(content);
    let mut scrim = div()
        .id(SharedString::from(format!("{id}/scrim")))
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .bg(palette.scrim().opacity(at.progress))
        .flex()
        .flex_col()
        .justify_end();
    if at.leaving {
        panel = panel.child(div().absolute().inset_0().occlude());
        return scrim.child(panel).into_any_element();
    }
    if at.drags {
        let pan = |begins| {
            cx.listener(move |root: &mut Root, event: &ScrollWheelEvent, _, cx| {
                root.sheet_panned(event, begins, cx)
            })
        };
        panel = panel.on_scroll_wheel(pan(true));
        scrim = scrim.on_scroll_wheel(pan(false));
    }
    scrim = scrim
        .occlude()
        .on_click(cx.listener(move |root, _, _, cx| close(root, cx)));
    scrim
        .child(ducktape_host::a11y::modal(panel))
        .into_any_element()
}

/// A sheet's head (Expo `SheetHead`): what the sheet is about, a hairline
/// under it.
pub fn head(palette: Palette) -> Div {
    div()
        .pt(px(look::gap::XXS))
        .pb(px(look::gap::LG))
        .px(px(look::SHEET_INSET))
        .border_b_1()
        .border_color(palette.border())
}

/// A sheet's head holding its title (17 medium, the boards' `shead`).
pub fn title(name: &'static str, palette: Palette) -> Div {
    head(palette)
        .child(look::text(look::TITLE, FontWeight::MEDIUM, palette.foreground()).child(name))
}

/// A 52 sheet row (Expo `SheetRow`): its icon 20 muted (its place kept
/// empty with none, so a list of choices lines up), its label, a hint at
/// the right.
pub fn row(
    id: impl Into<SharedString>,
    icon: Option<&'static str>,
    label: impl Into<SharedString>,
    hint: Option<SharedString>,
    palette: Palette,
) -> Stateful<Div> {
    let (id, label) = (id.into(), label.into());
    let icon = match icon {
        Some(icon) => svg()
            .path(icon)
            .size(px(look::ICON_SHEET))
            .flex_none()
            .text_color(palette.muted())
            .into_any_element(),
        None => div()
            .size(px(look::ICON_SHEET))
            .flex_none()
            .into_any_element(),
    };
    let mut row = div()
        .id(id.clone())
        .control(Role::Button, label.clone())
        .debug_selector(move || id.to_string())
        .h(px(look::SHEET_ROW))
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(look::SHEET_INSET))
        .active(|style| style.opacity(look::PRESSED))
        .child(icon)
        .child(
            look::text(look::BODY, FontWeight::NORMAL, palette.foreground())
                .flex_1()
                .min_w_0()
                .truncate()
                .child(label),
        );
    if let Some(hint) = hint {
        row = row.child(
            look::text(look::CAPTION, FontWeight::NORMAL, palette.muted())
                .flex_none()
                .child(hint),
        );
    }
    row
}

const SAMPLE: Duration = Duration::from_millis(16);

const RESTED: Duration = Duration::from_millis(40);

/// A finger dragging the sheet: how far down it has come from where the
/// drag began (points; up is less), its speed down (points per second)
/// over the last stretch of at least [`SAMPLE`], where that stretch
/// began, and when it last moved.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    down: f32,
    speed: f32,
    sampled: (Instant, f32),
    moved: Instant,
}

impl Drag {
    fn new(down: f32, now: Instant) -> Self {
        Self {
            down,
            speed: 0.,
            sampled: (now, down),
            moved: now,
        }
    }

    fn step(&mut self, step: f32, now: Instant) {
        self.down += step;
        if step != 0. {
            self.moved = now;
        }
        let (then, from) = self.sampled;
        let gone = now.saturating_duration_since(then);
        if gone >= SAMPLE {
            self.speed = (self.down - from) / gone.as_secs_f32();
            self.sampled = (now, self.down);
        }
    }

    fn thrown(&self, now: Instant) -> f32 {
        match now.saturating_duration_since(self.moved) > RESTED {
            true => 0.,
            false => self.speed,
        }
    }
}

impl Root {
    pub(super) fn sheet_panned(
        &mut self,
        event: &ScrollWheelEvent,
        begins: bool,
        cx: &mut Context<Self>,
    ) {
        let step = f32::from(event.delta.pixel_delta(px(1.)).y);
        let now = motion::now(cx);
        match (event.touch_phase, self.sheet_drag.as_mut()) {
            (TouchPhase::Started, _) => {
                let up = self
                    .drawn_sheet
                    .as_ref()
                    .is_some_and(|shown| !shown.leaving);
                if begins && up && step > 0. && !self.sheet_scrolled(cx) {
                    self.sheet_drag = Some(Drag::new(step, now));
                    cx.notify();
                }
            }
            (TouchPhase::Moved, Some(drag)) => {
                drag.step(step, now);
                cx.notify();
            }
            (TouchPhase::Ended, Some(drag)) => {
                drag.down += step;
                self.sheet_let_go(true, cx);
            }
            (TouchPhase::Cancelled, Some(_)) => self.sheet_let_go(false, cx),
            (TouchPhase::Moved | TouchPhase::Ended | TouchPhase::Cancelled, None) => {}
        }
    }

    fn sheet_scrolled(&self, cx: &App) -> bool {
        self.sheet == Some(Sheet::Switch) && self.join.read(cx).scroll.offset().y < px(0.)
    }

    fn sheet_let_go(&mut self, goes: bool, cx: &mut Context<Self>) {
        let Some(drag) = self.sheet_drag.take() else {
            return;
        };
        let height = self.sheet_height.get().map_or(1., f32::from).max(1.);
        let (down, speed) = (drag.down.max(0.), drag.thrown(motion::now(cx)));
        let closes = goes && swipe::crosses(down, speed, height);
        let from = 1. - (down / height).min(1.);
        if closes {
            self.close_sheet(cx);
        }
        cx.notify();
        if motion::reduced(cx) {
            return;
        }
        let to = if closes { 0. } else { 1. };
        if let Some(shown) = self.drawn_sheet.as_mut() {
            shown.tween = Tween::thrown(from, to, -speed / height, motion::idiom().sheet, cx);
            shown.leaving = closes;
        }
    }

    pub(super) fn sheet_held(&self, cx: &App) -> Option<f32> {
        let drag = self.sheet_drag?;
        let height = f32::from(self.sheet_height.get()?).max(1.);
        (!motion::reduced(cx)).then(|| 1. - (drag.down.max(0.) / height).min(1.))
    }
}
