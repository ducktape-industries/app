//! The page's bookkeeping, apart from WebKit so it is tested everywhere:
//! which life of the page is current, the calls in flight on it, and the
//! seatings waiting for it to be ready. It carries the host's dead-page
//! rules (`contract::ViewInstance`): every call into a dead page is
//! answered, the ones in flight when it died and every one begun on it
//! after; each call is answered once; a seating asked while a new page
//! loads waits for it rather than being refused.

use std::collections::HashMap;

pub(crate) type Answered = Result<String, String>;

pub(crate) struct Calls<A, W> {
    life: u64,
    state: State,
    next: u64,
    flying: HashMap<u64, A>,
    waiting: Vec<W>,
}

enum State {
    Loading,
    Ready,
    Failed(String),
}

pub(crate) enum Begun<A> {
    Send(u64),
    Dead(A),
}

pub(crate) enum Seating<W> {
    Run(W, u64),
    Held,
    Refused(W, String),
}

pub(crate) const DEAD: &str = "the page the view ran in died";

impl<A, W> Calls<A, W> {
    pub(crate) fn new() -> Self {
        Self {
            life: 0,
            state: State::Loading,
            next: 0,
            flying: HashMap::new(),
            waiting: Vec::new(),
        }
    }

    pub(crate) fn life(&self) -> u64 {
        self.life
    }

    pub(crate) fn begin(&mut self, life: u64, answer: A) -> Begun<A> {
        if life != self.life || !matches!(self.state, State::Ready) {
            return Begun::Dead(answer);
        }
        self.next += 1;
        self.flying.insert(self.next, answer);
        Begun::Send(self.next)
    }

    pub(crate) fn answered(&mut self, token: u64) -> Option<A> {
        self.flying.remove(&token)
    }

    pub(crate) fn when_ready(&mut self, work: W) -> Seating<W> {
        match &self.state {
            State::Ready => Seating::Run(work, self.life),
            State::Loading => {
                self.waiting.push(work);
                Seating::Held
            }
            State::Failed(why) => Seating::Refused(work, why.clone()),
        }
    }

    pub(crate) fn died(&mut self) -> Vec<A> {
        self.life += 1;
        self.state = State::Loading;
        self.flying.drain().map(|(_, answer)| answer).collect()
    }

    pub(crate) fn ready(&mut self) -> (Vec<W>, u64) {
        self.state = State::Ready;
        (std::mem::take(&mut self.waiting), self.life)
    }

    pub(crate) fn failed(&mut self, why: String) -> Vec<W> {
        self.state = State::Failed(why);
        std::mem::take(&mut self.waiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Test = Calls<&'static str, &'static str>;

    fn ready() -> Test {
        let mut calls = Test::new();
        let (held, life) = calls.ready();
        assert!(held.is_empty());
        assert_eq!(life, 0);
        calls
    }

    #[test]
    fn a_seating_while_the_page_loads_waits_for_it_and_runs_on_its_life() {
        let mut calls = Test::new();
        assert!(matches!(calls.when_ready("seat a"), Seating::Held));
        assert_eq!(calls.ready(), (vec!["seat a"], 0));
        assert!(matches!(
            calls.when_ready("seat b"),
            Seating::Run("seat b", 0)
        ));
    }

    #[test]
    fn every_call_into_a_dead_page_is_answered_once() {
        let mut calls = ready();
        let Begun::Send(first) = calls.begin(0, "tick in flight") else {
            panic!("not sent");
        };
        assert_eq!(calls.died(), vec!["tick in flight"]);
        assert_eq!(calls.answered(first), None, "answered twice");
        assert!(matches!(
            calls.begin(0, "tick after"),
            Begun::Dead("tick after")
        ));
        assert!(matches!(calls.when_ready("revive"), Seating::Held));
        assert_eq!(calls.ready(), (vec!["revive"], 1));
        assert!(matches!(calls.begin(0, "old"), Begun::Dead("old")));
        let Begun::Send(token) = calls.begin(1, "new") else {
            panic!("not sent");
        };
        assert_eq!(calls.answered(token), Some("new"));
        assert_eq!(calls.answered(token), None);
    }

    #[test]
    fn a_page_that_did_not_load_refuses_every_seating() {
        let mut calls = Test::new();
        assert!(matches!(calls.when_ready("held"), Seating::Held));
        assert_eq!(calls.failed("no page".into()), vec!["held"]);
        assert!(matches!(
            calls.when_ready("later"),
            Seating::Refused("later", why) if why == "no page"
        ));
    }
}
