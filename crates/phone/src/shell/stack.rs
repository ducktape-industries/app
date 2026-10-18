//! Where the person is (wasm-mobile SPEC §7.1), and what they have open:
//! one of four tab roots (Home, Open, Notifications, Account), the
//! programs pushed over it, and every program opened since, each in its
//! one seat, kept warm where it was left. Tabs show on the roots only; a
//! pushed program fills the screen under its capsule.
//! The shape is gpui-mobile's example router (`is_tab_root`, a history
//! pushed by `navigate_to`, `example/src/screens/mod.rs:132,191,288`).
//!
//! It lives as long as the process, not the window: an Android Activity
//! made again finds the stack and the seats as they were (SPEC §7.5).

use std::time::Instant;

/// A tab root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    /// The network's programs.
    #[default]
    Home,
    /// The programs already open, where they were left.
    Open,
    /// The notices the programs posted, newest first.
    Notifications,
    /// Who is reading, and the way to the account, App settings and
    /// Switch network.
    Account,
}

impl Tab {
    /// The tabs as the bar lists them, left to right.
    const BAR: [Tab; 4] = [Tab::Home, Tab::Open, Tab::Notifications, Tab::Account];

    /// The tab beside this one in the bar: the one to its right when
    /// `right`, else the one to its left; none past either end.
    pub fn beside(self, right: bool) -> Option<Tab> {
        let at = Self::BAR.iter().position(|tab| *tab == self)?;
        let at = match right {
            true => at + 1,
            false => at.checked_sub(1)?,
        };
        Self::BAR.get(at).copied()
    }
}

/// A program opened, in its one seat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opened {
    /// The program.
    pub program: &'static str,
    /// Its seat's layout instance.
    pub instance: u64,
    /// When it last left the screen; `None` while it is on the stack.
    pub left: Option<Instant>,
}

/// The stack.
#[derive(Debug)]
pub struct Stack {
    tab: Tab,
    pushed: Vec<&'static str>,
    /// Every program opened, the latest first.
    open: Vec<Opened>,
    /// The layout instance the next program opened gets.
    next: u64,
}

impl Default for Stack {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            pushed: Vec::new(),
            open: Vec::new(),
            next: 1,
        }
    }
}

impl Stack {
    /// The tab root under everything pushed.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The program on top, `None` at a tab root.
    pub fn top(&self) -> Option<&'static str> {
        self.pushed.last().copied()
    }

    /// The program under the top one, `None` when the top one is on a root.
    pub fn under(&self) -> Option<&'static str> {
        self.pushed.iter().rev().nth(1).copied()
    }

    /// Every program opened, the latest first.
    pub fn open(&self) -> &[Opened] {
        &self.open
    }

    /// The seat instance of `program`, if it is open.
    pub fn instance(&self, program: &str) -> Option<u64> {
        self.open
            .iter()
            .find(|opened| opened.program == program)
            .map(|opened| opened.instance)
    }

    /// Moves to `tab`'s root. Only a root shows the tabs, so nothing is
    /// pushed when one is picked.
    pub fn pick(&mut self, tab: Tab, now: Instant) {
        self.tab = tab;
        self.leave_all(now);
    }

    /// `program` on top, in its seat (a new one the first time, the one it
    /// was left in after): a program already on top is not pushed twice.
    /// Its seat instance.
    pub fn push(&mut self, program: &'static str) -> u64 {
        let instance = match self
            .open
            .iter()
            .position(|opened| opened.program == program)
        {
            Some(at) => {
                let mut opened = self.open.remove(at);
                opened.left = None;
                self.open.insert(0, opened);
                opened.instance
            }
            None => {
                let instance = self.next;
                self.next += 1;
                self.open.insert(
                    0,
                    Opened {
                        program,
                        instance,
                        left: None,
                    },
                );
                instance
            }
        };
        if self.top() != Some(program) {
            self.pushed.push(program);
        }
        instance
    }

    /// Back: the top program goes (its seat kept); at a root other than
    /// Home, Home; at Home, nothing, and `false` says so (the OS then does
    /// what it does there).
    pub fn back(&mut self, now: Instant) -> bool {
        if let Some(program) = self.pushed.pop() {
            self.left(program, now);
            return true;
        }
        if self.tab != Tab::Home {
            self.tab = Tab::Home;
            return true;
        }
        false
    }

    /// `program` closed from Open: its seat goes, and it stays on Home.
    pub fn close(&mut self, program: &str) {
        self.open.retain(|opened| opened.program != program);
        self.pushed.retain(|pushed| *pushed != program);
    }

    /// The OS is short of memory: every program not on the stack is let go
    /// (SPEC §7.5).
    pub fn trim(&mut self) {
        let pushed = &self.pushed;
        self.open.retain(|opened| pushed.contains(&opened.program));
    }

    /// The network was left: nothing is open, and Home is a new network's.
    pub fn clear(&mut self) {
        self.pushed.clear();
        self.open.clear();
        self.tab = Tab::Home;
    }

    fn leave_all(&mut self, now: Instant) {
        for program in std::mem::take(&mut self.pushed) {
            self.left(program, now);
        }
    }

    fn left(&mut self, program: &str, now: Instant) {
        if self.pushed.contains(&program) {
            return;
        }
        if let Some(opened) = self
            .open
            .iter_mut()
            .find(|opened| opened.program == program)
        {
            opened.left = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Back pops a program (its seat kept, marked left), then leaves a
    /// non-Home tab (any of the other three) for Home, then is refused at
    /// Home.
    #[test]
    fn back_pops_then_goes_home_then_stops() {
        let now = Instant::now();
        let mut stack = Stack::default();
        for tab in [Tab::Notifications, Tab::Account] {
            stack.pick(tab, now);
            assert!(stack.back(now), "Back at {tab:?} was refused");
            assert_eq!(stack.tab(), Tab::Home, "Back at {tab:?}");
        }
        stack.pick(Tab::Open, now);
        let chat = stack.push("chat");
        assert_eq!(stack.push("chat"), chat, "one seat per program");
        assert!(stack.back(now));
        assert_eq!((stack.tab(), stack.top()), (Tab::Open, None));
        assert_eq!(stack.open()[0].left, Some(now));
        assert!(stack.back(now));
        assert_eq!(stack.tab(), Tab::Home);
        assert!(!stack.back(now));
    }

    /// Each tab's neighbours are the bar's, and nothing is past its ends.
    #[test]
    fn a_tab_has_the_bars_neighbours_and_none_past_its_ends() {
        assert_eq!(Tab::Home.beside(false), None);
        assert_eq!(Tab::Home.beside(true), Some(Tab::Open));
        assert_eq!(Tab::Open.beside(false), Some(Tab::Home));
        assert_eq!(Tab::Notifications.beside(true), Some(Tab::Account));
        assert_eq!(Tab::Account.beside(true), None);
    }

    /// The capsule's × lands on Home with the programs still open, the
    /// latest first; one opened again comes back in the seat it left.
    #[test]
    fn home_keeps_the_programs_open_in_their_seats() {
        let now = Instant::now();
        let mut stack = Stack::default();
        let chat = stack.push("chat");
        let forge = stack.push("forge");
        stack.pick(Tab::Home, now);
        assert_eq!((stack.tab(), stack.top()), (Tab::Home, None));
        let open: Vec<_> = stack.open().iter().map(|it| it.program).collect();
        assert_eq!(open, ["forge", "chat"]);
        assert_ne!(chat, forge);
        assert_eq!(stack.push("chat"), chat);
        assert_eq!(stack.open()[0].left, None);
    }

    /// Memory pressure lets go of what is not on the stack; closing one
    /// from Open lets it go; leaving the network lets go of all.
    #[test]
    fn trim_close_and_clear_let_seats_go() {
        let now = Instant::now();
        let mut stack = Stack::default();
        stack.push("chat");
        stack.pick(Tab::Home, now);
        stack.push("forge");
        stack.push("explorer");
        stack.trim();
        let open: Vec<_> = stack.open().iter().map(|it| it.program).collect();
        assert_eq!(open, ["explorer", "forge"]);
        stack.close("forge");
        assert_eq!(stack.instance("forge"), None);
        assert_eq!(stack.top(), Some("explorer"));
        stack.clear();
        assert!(stack.open().is_empty());
        assert_eq!(stack.top(), None);
    }
}
