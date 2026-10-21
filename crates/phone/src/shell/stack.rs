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

use ducktape_host::runtime::intern;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Instant;

/// A tab root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    /// The network's programs.
    #[default]
    Home,
    /// The programs already open, where they were left.
    Open,
    /// The notices the programs posted, newest first.
    Notifications,
    /// Who is reading, the way to the account, this phone, the network
    /// and the appearance.
    Account,
}

impl Tab {
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
    /// When it last left the screen in this run; `None` while it is on the
    /// stack, and for a program listed from the last launch until it is.
    pub left: Option<Instant>,
    /// Whether its seat is made: a program kept from the last launch is
    /// listed unseated until it is pushed.
    pub seated: bool,
}

/// What the stack was on a network, as `phone.json` keeps it across
/// launches: no seat, no time, no view's state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// The tab root under everything pushed.
    pub tab: Tab,
    /// The programs pushed over it, the bottom first.
    pub pushed: Vec<String>,
    /// Every program open, the latest first.
    pub open: Vec<KeptOpen>,
}

/// A program open, as kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptOpen {
    /// The program.
    pub program: String,
    /// The route its view last showed, if it named one.
    pub showing: Option<String>,
}

/// The stack.
#[derive(Debug)]
pub struct Stack {
    tab: Tab,
    pushed: Vec<&'static str>,
    open: Vec<Opened>,
    next: u64,
    showing: BTreeMap<&'static str, String>,
}

impl Default for Stack {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            pushed: Vec::new(),
            open: Vec::new(),
            next: 1,
            showing: BTreeMap::new(),
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
                opened.seated = true;
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
                        seated: true,
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
        self.showing.remove(program);
    }

    /// The OS is short of memory: every seat not on the stack is let go
    /// (SPEC §7.5); a program listed unseated holds none and stays listed.
    pub fn trim(&mut self) {
        let pushed = &self.pushed;
        self.open
            .retain(|opened| !opened.seated || pushed.contains(&opened.program));
        let open = &self.open;
        self.showing
            .retain(|program, _| open.iter().any(|opened| opened.program == *program));
    }

    /// The network was left: nothing is open, and Home is a new network's.
    pub fn clear(&mut self) {
        self.pushed.clear();
        self.open.clear();
        self.showing.clear();
        self.tab = Tab::Home;
    }

    /// The route the view in seat `instance` shows now, kept for the next
    /// launch; whether it moved.
    pub fn set_showing(&mut self, instance: u64, route: &str) -> bool {
        let Some(program) = self
            .open
            .iter()
            .find(|opened| opened.instance == instance)
            .map(|opened| opened.program)
        else {
            return false;
        };
        if self.showing.get(program).map(String::as_str) == Some(route) {
            return false;
        }
        self.showing.insert(program, route.to_owned());
        true
    }

    /// The route `program`'s view showed when it was kept, while it is
    /// listed from the last launch and not seated yet: its seat is handed it
    /// as it is made.
    pub fn waking(&self, program: &str) -> Option<String> {
        let opened = self.open.iter().find(|opened| opened.program == program)?;
        if opened.seated {
            return None;
        }
        self.showing.get(program).cloned()
    }

    /// The stack as it is kept across launches.
    pub fn kept(&self) -> Kept {
        let open = self.open.iter().map(|opened| KeptOpen {
            program: opened.program.to_owned(),
            showing: self.showing.get(opened.program).cloned(),
        });
        Kept {
            tab: self.tab,
            pushed: self
                .pushed
                .iter()
                .map(|program| (*program).to_owned())
                .collect(),
            open: open.collect(),
        }
    }

    /// `kept` taken up into a stack with nothing open: its tab, every
    /// program it had open in its order, each once, and those pushed over
    /// the tab pushed again and seated; the rest listed unseated, each
    /// seated when it is next pushed. No age comes back: one starts when it
    /// is left in this run. The route each seated program's view last
    /// showed, by its seat, to hand it.
    pub fn take_up(&mut self, kept: &Kept) -> Vec<(&'static str, u64, String)> {
        let pushed: Vec<&'static str> = kept
            .pushed
            .iter()
            .map(|program| intern(program.as_str()))
            .collect();
        for open in &kept.open {
            let program = intern(open.program.as_str());
            if self.instance(program).is_some() {
                continue;
            }
            let seated = pushed.contains(&program);
            let instance = self.next;
            self.next += 1;
            self.open.push(Opened {
                program,
                instance,
                left: None,
                seated,
            });
            let showing = open.showing.clone().filter(|showing| !showing.is_empty());
            if let Some(showing) = showing {
                self.showing.insert(program, showing);
            }
        }
        self.tab = kept.tab;
        let open = &self.open;
        self.pushed = pushed
            .into_iter()
            .filter(|program| open.iter().any(|opened| opened.program == *program))
            .collect();
        let seated = self.open.iter().filter(|opened| opened.seated);
        seated
            .filter_map(|opened| {
                let route = self.showing.get(opened.program)?.clone();
                Some((opened.program, opened.instance, route))
            })
            .collect()
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

    #[test]
    fn a_tab_has_the_bars_neighbours_and_none_past_its_ends() {
        assert_eq!(Tab::Home.beside(false), None);
        assert_eq!(Tab::Home.beside(true), Some(Tab::Open));
        assert_eq!(Tab::Open.beside(false), Some(Tab::Home));
        assert_eq!(Tab::Notifications.beside(true), Some(Tab::Account));
        assert_eq!(Tab::Account.beside(true), None);
    }

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

    #[test]
    fn a_kept_stack_comes_back_with_only_the_pushed_seated() {
        let now = Instant::now();
        let mut stack = Stack::default();
        let files = stack.push("files");
        stack.back(now);
        let chat = stack.push("chat");
        assert!(stack.set_showing(chat, "general"));
        assert!(!stack.set_showing(chat, "general"), "the same route again");
        assert!(stack.set_showing(files, "home/tech"));
        let kept = stack.kept();
        assert_eq!(
            serde_json::to_string(&kept).expect("JSON"),
            r#"{"tab":"home","pushed":["chat"],"open":[{"program":"chat","showing":"general"},{"program":"files","showing":"home/tech"}]}"#
        );

        let mut next = Stack::default();
        let routes = next.take_up(&kept);
        assert_eq!((next.tab(), next.top()), (Tab::Home, Some("chat")));
        let open: Vec<_> = next
            .open()
            .iter()
            .map(|it| (it.program, it.seated))
            .collect();
        assert_eq!(open, [("chat", true), ("files", false)]);
        let left: Vec<_> = next.open().iter().map(|it| it.left).collect();
        assert_eq!(left, [None, None], "no age comes back from the last launch");
        let chat = next.instance("chat").expect("listed");
        assert_eq!(routes, [("chat", chat, "general".to_owned())]);
        assert_eq!(next.waking("chat"), None, "seated already");
        assert_eq!(next.waking("files"), Some("home/tech".to_owned()));
        next.trim();
        assert!(
            next.waking("files").is_some(),
            "a trim let an unseated program go"
        );
        let files = next.push("files");
        assert_eq!(next.waking("files"), None, "pushed, it is seated");
        assert!(next.open()[0].seated);
        assert_ne!(files, chat);
        let kept = next.kept();
        assert_eq!(kept.pushed, ["chat", "files"]);
        let open: Vec<_> = kept.open.iter().map(|it| it.program.as_str()).collect();
        assert_eq!(open, ["files", "chat"]);
        let older: Kept =
            serde_json::from_str(r#"{"pushed":["chat"]}"#).expect("a record missing fields");
        assert_eq!((older.tab, older.open.len()), (Tab::Home, 0));
    }

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
