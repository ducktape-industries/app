//! This device's preferences as the shell draws them. Its methods are its
//! one writer: each saves to `prefs.json` (`backend::prefs`,
//! `runtime::notify::settings`) and then edits the compared value, so a
//! choice already made notifies nobody.
use super::Slice;
use crate::backend::{self, Appearance, Layout};
use crate::runtime::notify;
use gpui_kit::Context;

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub appearance: Appearance,
    /// The OS says dark; counts under `Appearance::System`.
    pub system_dark: bool,
    pub motion: bool,
    /// Where the programs are listed; `None` until the launcher's last
    /// step asked (`Account::show` asks that); drawn through
    /// `layout_or_default`.
    pub layout: Option<Layout>,
    /// Read off disk once, and again only after a notice setting was saved.
    pub notify: notify::Settings,
    /// Debug mode (the phone's App settings): the perf counters on
    /// (`perf::switch`) and what each program costs shown. The shell
    /// switches the counters on at launch when it is.
    pub debug: bool,
    pub text_size: TextSize,
}

pub const DYNAMIC_TYPE_BODY: [u64; 12] = [14, 15, 16, 17, 19, 21, 23, 28, 33, 40, 47, 53];

const DYNAMIC_TYPE_DEFAULT: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSize(usize);

impl Default for TextSize {
    fn default() -> Self {
        Self(DYNAMIC_TYPE_DEFAULT)
    }
}

impl gpui_kit::Global for TextSize {}

impl TextSize {
    pub fn of_points(points: Option<u64>) -> Self {
        points
            .and_then(|points| DYNAMIC_TYPE_BODY.iter().position(|body| *body == points))
            .map_or_else(Self::default, Self)
    }

    pub fn points(self) -> u64 {
        DYNAMIC_TYPE_BODY[self.0]
    }

    pub fn larger(self) -> Self {
        Self((self.0 + 1).min(DYNAMIC_TYPE_BODY.len() - 1))
    }

    pub fn smaller(self) -> Self {
        Self(self.0.saturating_sub(1))
    }

    pub fn is_default(self) -> bool {
        self.0 == DYNAMIC_TYPE_DEFAULT
    }

    pub fn scale(self) -> f32 {
        self.points() as f32 / DYNAMIC_TYPE_BODY[DYNAMIC_TYPE_DEFAULT] as f32
    }
}

pub fn text_scale(cx: &gpui_kit::App) -> Option<f32> {
    cx.try_global::<TextSize>()
        .filter(|size| !size.is_default())
        .map(|size| size.scale())
}

impl Prefs {
    /// As the prefs file has them; the theme's own word comes with
    /// `Windows::sync_appearance`.
    pub fn load() -> Self {
        Self {
            appearance: backend::load_appearance(),
            system_dark: false,
            motion: backend::load_motion(),
            layout: backend::load_layout(),
            notify: notify::Settings::load(),
            debug: backend::load_debug(),
            text_size: TextSize::of_points(backend::load_text_size()),
        }
    }

    pub fn dark(&self) -> bool {
        match self.appearance {
            Appearance::Light => false,
            Appearance::Dark => true,
            Appearance::System => self.system_dark,
        }
    }

    /// The layout to draw: not chosen yet draws the menu bar (the desk
    /// never waits on the choice).
    pub fn layout_or_default(&self) -> Layout {
        self.layout.unwrap_or_default()
    }
}

fn timed() -> Option<crate::perf::Timer> {
    crate::perf::time(crate::perf::Key::Shell, "reducer.desk")
}

impl Slice<Prefs> {
    pub fn set_appearance(&mut self, mode: Appearance, cx: &mut Context<Self>) {
        let _timed = timed();
        backend::save_appearance(mode);
        self.edit(|prefs| prefs.appearance = mode, cx);
    }

    pub fn set_motion(&mut self, on: bool, cx: &mut Context<Self>) {
        let _timed = timed();
        backend::save_motion(on);
        self.edit(|prefs| prefs.motion = on, cx);
    }

    pub fn set_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        let _timed = timed();
        backend::save_layout(layout);
        self.edit(|prefs| prefs.layout = Some(layout), cx);
    }

    /// Debug mode on or off, and the perf counters with it.
    pub fn set_debug(&mut self, on: bool, cx: &mut Context<Self>) {
        let _timed = timed();
        backend::save_debug(on);
        crate::perf::switch(on);
        self.edit(|prefs| prefs.debug = on, cx);
    }

    pub fn set_text_size(&mut self, size: TextSize, cx: &mut Context<Self>) {
        let _timed = timed();
        backend::save_text_size((!size.is_default()).then(|| size.points()));
        cx.set_global(size);
        cx.refresh_windows();
        self.edit(|prefs| prefs.text_size = size, cx);
    }

    /// What the theme says the OS is: written by whoever syncs the theme.
    pub fn set_system_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
        self.edit(|prefs| prefs.system_dark = dark, cx);
    }

    pub fn set_notify_banners(&mut self, on: bool, cx: &mut Context<Self>) {
        let _timed = timed();
        notify::save_banners(on);
        self.reload_notify(cx);
    }

    pub fn set_notify_in_front(&mut self, show: bool, cx: &mut Context<Self>) {
        let _timed = timed();
        notify::save_in_front(show);
        self.reload_notify(cx);
    }

    pub fn set_notify_burst(&mut self, burst: u32, cx: &mut Context<Self>) {
        let _timed = timed();
        notify::save_burst(burst);
        self.reload_notify(cx);
    }

    /// The notice settings read off disk again: after a view's permission
    /// was saved (`Notifications::permission` writes the file).
    pub fn reload_notify(&mut self, cx: &mut Context<Self>) {
        let notify = notify::Settings::load();
        self.edit(|prefs| prefs.notify = notify, cx);
    }

    /// Everything read off disk again: the door's walk put the prefs file
    /// back (`layers::Kept::restore`).
    #[cfg(any(test, feature = "ax-door"))]
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let _timed = timed();
        let loaded = Prefs::load();
        self.edit(
            |prefs| {
                prefs.appearance = loaded.appearance;
                prefs.motion = loaded.motion;
                prefs.layout = loaded.layout;
                prefs.notify = loaded.notify;
                prefs.debug = loaded.debug;
                prefs.text_size = loaded.text_size;
            },
            cx,
        );
        cx.set_global(loaded.text_size);
        cx.refresh_windows();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_size_steps_through_dynamic_type_and_stops_at_its_ends() {
        let default = TextSize::default();
        assert_eq!((default.points(), default.scale()), (17, 1.));
        assert_eq!(default.larger().points(), 19);
        assert_eq!(default.smaller().points(), 16);
        let mut top = default;
        for _ in 0..DYNAMIC_TYPE_BODY.len() {
            top = top.larger();
        }
        assert_eq!(top.points(), 53);
        let mut bottom = default;
        for _ in 0..DYNAMIC_TYPE_BODY.len() {
            bottom = bottom.smaller();
        }
        assert_eq!(bottom.points(), 14);
        assert_eq!(TextSize::of_points(Some(28)).points(), 28);
        assert_eq!(TextSize::of_points(Some(18)), default, "not a step");
        assert_eq!(TextSize::of_points(None), default);
    }
}
