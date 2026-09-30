//! This device's key and the account it holds, as the sign-in screens and
//! the bar draw them. Secrets never enter it: a typed password or phrase
//! stays in the field that shows it. Bridged from the model until s10
//! moves the sign-in flows here.

/// The account's compared value.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Account {
    /// The seated key's public half, hex; empty while locked.
    pub(crate) signer_key: String,
    /// `None` not asked yet; `Some(None)` no account; `Some(Some(..))` found.
    pub(crate) account: Option<Option<(u64, String)>>,
    /// A password-locked key file waits for its password.
    pub(crate) key_exists: bool,
    pub(crate) locked: bool,
    pub(crate) seating: bool,
    /// A sign-in call is out.
    pub(crate) busy: bool,
    /// The current sign-in step's failure.
    pub(crate) error: String,
    /// "Add a device…": the joining key's fingerprint, once found.
    pub(crate) approve: Option<String>,
    /// The code this device waits under for another to approve it.
    pub(crate) link_code: String,
    pub(crate) passkey_waiting: bool,
    /// The passkey QR URL, once the person picked the phone.
    pub(crate) passkey_qr: Option<String>,
    /// Help greets a new account rather than titling itself.
    pub(crate) welcome: bool,
}
