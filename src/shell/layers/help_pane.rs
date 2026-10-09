//! Help in a pane: the app's own page (`help::help_view`), a view of its
//! own so the pane's body is cached apart from the window around it.

use super::super::entities::{Account, Prefs, Slice};
use super::super::{WindowKey, help};
use gpui_kit::{App, Context, Entity, IntoElement, Render, Subscription, Window};

/// One Help pane's body. It draws from `welcome` (`Account`), `dark` and
/// the layout (`Prefs`), copied by its observers and compared, so a move of
/// either that leaves all three be draws nothing.
pub(in crate::shell) struct HelpPane {
    key: WindowKey,
    account: Entity<Account>,
    prefs: Entity<Slice<Prefs>>,
    shown: (bool, bool, bool),
    _observing: [Subscription; 2],
}

impl HelpPane {
    pub(in crate::shell) fn new(
        account: &Entity<Account>,
        prefs: &Entity<Slice<Prefs>>,
        key: WindowKey,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            key,
            account: account.clone(),
            prefs: prefs.clone(),
            shown: (false, false, false),
            _observing: [
                cx.observe(account, |this, _, cx| this.moved(cx)),
                cx.observe(prefs, |this, _, cx| this.moved(cx)),
            ],
        };
        this.shown = this.now(cx);
        this
    }

    /// What it shows: the welcome, the dark page, the sidebar's words.
    fn now(&self, cx: &App) -> (bool, bool, bool) {
        let prefs = self.prefs.read(cx).get();
        (
            self.account.read(cx).get().welcome,
            prefs.dark(),
            prefs.layout_or_default() == crate::backend::Layout::Sidebar,
        )
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        let now = self.now(cx);
        if now != self.shown {
            self.shown = now;
            cx.notify();
        }
    }
}

impl Render for HelpPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.help", 1);
        let (welcome, dark, side) = self.shown;
        help::help_view(welcome, dark, side)
    }
}
