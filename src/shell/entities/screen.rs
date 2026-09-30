//! Which screen the console shows: a launcher step, or the desk. Bridged
//! from the model's `Stage` until s10's methods write it.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Screen {
    #[default]
    Connect,
    /// The key step; `awaiting` the node's word on its account.
    Unlock {
        awaiting: bool,
    },
    /// A new recovery key's words, then (`quiz`) the three typed back.
    Phrase {
        quiz: Option<[usize; 3]>,
    },
    Recover,
    Account {
        step: AccountStep,
    },
    Desk,
}

/// The account step's sub-step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AccountStep {
    /// Naming a new account.
    Name,
    /// Showing a code for another device to approve.
    Link,
    /// A passkey ceremony in the browser.
    Passkey,
}
