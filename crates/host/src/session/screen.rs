//! Which screen the console shows: a launcher step, or the desk. Written
//! by `Session`'s and `Account`'s methods.

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
    /// Where the programs sit on this device: asked once, in place of the
    /// desk, while no layout is chosen (`Account::show`).
    Layout,
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
