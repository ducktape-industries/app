//! The keeps handed to the OS's own viewer, by ticket, until each answers: one the host lets go
//! of before the OS shows it answers not kept at once, and the OS never shows it.
use ducktape_host::contract::KeepDone;
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) static KEEPS: Keeps = Keeps::new();

pub(crate) struct Keeps(Mutex<Table>);

struct Table {
    next_ticket: u64,
    waiting: BTreeMap<u64, Waiting>,
}

struct Waiting {
    done: KeepDone,
    shown: bool,
}

impl Keeps {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(Table {
            next_ticket: 0,
            waiting: BTreeMap::new(),
        }))
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn hand(&self, done: KeepDone) -> u64 {
        let mut table = self.table();
        let ticket = table.next_ticket;
        table.next_ticket += 1;
        table.waiting.insert(ticket, Waiting { done, shown: false });
        ticket
    }

    pub(crate) fn shows(&self, ticket: u64) -> bool {
        match self.table().waiting.get_mut(&ticket) {
            Some(waiting) => {
                waiting.shown = true;
                true
            }
            None => false,
        }
    }

    pub(crate) fn unkeep(&self, ticket: u64) {
        let unshown = {
            let mut table = self.table();
            let shown = table
                .waiting
                .get(&ticket)
                .is_none_or(|waiting| waiting.shown);
            match shown {
                true => None,
                false => table.waiting.remove(&ticket),
            }
        };
        if let Some(waiting) = unshown {
            (waiting.done)(Ok(false));
        }
    }

    pub(crate) fn answer(&self, ticket: u64, answer: Result<bool, String>) {
        let waiting = self.table().waiting.remove(&ticket);
        if let Some(waiting) = waiting {
            (waiting.done)(answer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    type Answers = Arc<Mutex<Vec<Result<bool, String>>>>;

    fn heard() -> (Answers, KeepDone) {
        let answers = Arc::new(Mutex::new(Vec::new()));
        let told = answers.clone();
        let done: KeepDone = Box::new(move |answer| {
            told.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(answer)
        });
        (answers, done)
    }

    fn answers_of(answers: &Mutex<Vec<Result<bool, String>>>) -> Vec<Result<bool, String>> {
        answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    #[test]
    fn a_keep_let_go_after_the_hand_off_and_before_the_os_shows_it_never_shows() {
        let keeps = Keeps::new();
        let (answers, done) = heard();
        let ticket = keeps.hand(done);
        keeps.unkeep(ticket);
        assert_eq!(
            answers_of(&answers),
            [Ok(false)],
            "answered not kept at once"
        );
        assert!(!keeps.shows(ticket), "the queued viewer opens nothing");
        keeps.answer(ticket, Ok(true));
        assert_eq!(answers_of(&answers), [Ok(false)], "answered once");
    }

    #[test]
    fn a_keep_let_go_once_the_os_shows_it_answers_when_the_viewer_closes() {
        let keeps = Keeps::new();
        let (answers, done) = heard();
        let ticket = keeps.hand(done);
        assert!(keeps.shows(ticket));
        keeps.unkeep(ticket);
        assert!(
            answers_of(&answers).is_empty(),
            "a viewer that shows still reads the file"
        );
        keeps.answer(ticket, Ok(true));
        assert_eq!(answers_of(&answers), [Ok(true)]);
        keeps.unkeep(ticket);
        assert_eq!(answers_of(&answers), [Ok(true)], "answered once");
    }

    #[test]
    fn each_keep_has_its_own_ticket() {
        let keeps = Keeps::new();
        let (first_answers, first) = heard();
        let (second_answers, second) = heard();
        let (one, two) = (keeps.hand(first), keeps.hand(second));
        assert_ne!(one, two);
        keeps.unkeep(one);
        assert!(keeps.shows(two), "the other keep is still wanted");
        assert_eq!(answers_of(&first_answers), [Ok(false)]);
        assert!(answers_of(&second_answers).is_empty());
    }
}
