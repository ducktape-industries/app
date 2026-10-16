//! Where the person is (wasm-mobile SPEC §7.1): one of three tab roots
//! (Home, Open, Account), and the screens pushed over it. Tabs show on the
//! roots only; a pushed program fills the screen under its capsule. The
//! shape is gpui-mobile's example router (`is_tab_root`, a history pushed
//! by `navigate_to`, `example/src/screens/mod.rs:132,191,288`).

/// A tab root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    /// The network's programs.
    #[default]
    Home,
    /// The programs already open, where they were left.
    Open,
    /// The account: the `programs` program's view.
    Account,
}

/// A screen pushed over a tab root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pushed {
    /// A program, full screen under its capsule.
    Program(&'static str),
}

/// The stack.
#[derive(Debug, Default)]
pub struct Stack {
    tab: Tab,
    pushed: Vec<Pushed>,
}

impl Stack {
    /// The tab root under everything pushed.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The screen on top, `None` at a tab root.
    pub fn top(&self) -> Option<Pushed> {
        self.pushed.last().copied()
    }

    /// Moves to `tab`'s root. Only a root shows the tabs, so nothing is
    /// pushed when one is picked.
    pub fn pick(&mut self, tab: Tab) {
        self.tab = tab;
        self.pushed.clear();
    }

    /// Puts `screen` on top; a program already on top is not pushed twice.
    pub fn push(&mut self, screen: Pushed) {
        if self.top() != Some(screen) {
            self.pushed.push(screen);
        }
    }

    /// Back: the top screen goes; at a root other than Home, Home; at Home,
    /// nothing, and `false` says so (the OS then does what it does there).
    pub fn back(&mut self) -> bool {
        if self.pushed.pop().is_some() {
            return true;
        }
        if self.tab != Tab::Home {
            self.tab = Tab::Home;
            return true;
        }
        false
    }

    /// Back to the tab root, whatever is pushed (a capsule's close).
    pub fn close(&mut self) {
        self.pushed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Back pops a program, then leaves a non-Home tab for Home, then is
    /// refused at Home.
    #[test]
    fn back_pops_then_goes_home_then_stops() {
        let mut stack = Stack::default();
        stack.pick(Tab::Open);
        stack.push(Pushed::Program("chat"));
        stack.push(Pushed::Program("chat"));
        assert!(stack.back());
        assert_eq!((stack.tab(), stack.top()), (Tab::Open, None));
        assert!(stack.back());
        assert_eq!(stack.tab(), Tab::Home);
        assert!(!stack.back());
    }

    /// Close drops every pushed screen; picking a tab starts at its root.
    #[test]
    fn close_and_pick_land_on_a_root() {
        let mut stack = Stack::default();
        stack.push(Pushed::Program("chat"));
        stack.push(Pushed::Program("forge"));
        stack.close();
        assert_eq!(stack.top(), None);
        stack.push(Pushed::Program("chat"));
        stack.pick(Tab::Account);
        assert_eq!((stack.tab(), stack.top()), (Tab::Account, None));
    }
}
