//! Help in a pane: the app's own page (`help::help_view`), a view of its
//! own so the pane's body is cached apart from the window around it.

use super::super::{Desktop, WindowKey, help};
use gpui_kit::{Context, Entity, IntoElement, Render, Subscription, Window};

/// One Help pane's body. It draws from `welcome` and `dark`, copied from
/// the model by its observer (the bridge until `Account` and `Prefs` are
/// entities) and compared, so a dispatch that moves neither leaves it be.
pub(in crate::shell) struct HelpPane {
    key: WindowKey,
    welcome: bool,
    dark: bool,
    _observing: Subscription,
}

impl HelpPane {
    pub(in crate::shell) fn new(
        model: &Entity<Desktop>,
        key: WindowKey,
        cx: &mut Context<Self>,
    ) -> Self {
        let of = |model: &Desktop| (model.state.welcome, model.state.dark());
        let (welcome, dark) = of(model.read(cx));
        Self {
            key,
            welcome,
            dark,
            _observing: cx.observe(model, move |this, model, cx| {
                let now = of(model.read(cx));
                if now != (this.welcome, this.dark) {
                    (this.welcome, this.dark) = now;
                    cx.notify();
                }
            }),
        }
    }
}

impl Render for HelpPane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        crate::perf::count(crate::perf::Key::Window(self.key), "renders.help", 1);
        help::help_view(self.welcome, self.dark)
    }
}
