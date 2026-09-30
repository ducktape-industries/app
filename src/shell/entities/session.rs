//! The node in hand as the shell draws it: reached or on its way, which
//! network, what was typed, what went wrong. No status line: "Connected ·
//! block N" moved with every block, and the screen that says it builds it
//! from `Chain`'s height. Bridged from the model until s10 moves the
//! connect flow here.

/// The session's compared value.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Session {
    /// A node answered and is polled; true through a switch in flight.
    pub(crate) connected: bool,
    /// A connect attempt is out.
    pub(crate) connecting: bool,
    /// Connected, but the node stopped answering its status polls.
    pub(crate) reconnecting: bool,
    /// The node the views are on.
    pub(crate) connected_rpc: String,
    pub(crate) network: String,
    /// The chain id `duck://` links carry.
    pub(crate) chain: String,
    /// The node address being typed or tried.
    pub(crate) endpoint: String,
    pub(crate) endpoint_error: String,
    pub(crate) recent_endpoints: Vec<crate::backend::RecentEndpoint>,
    /// The network shares its name with another chain met first.
    pub(crate) other_chain: bool,
    /// The last connect attempt's failure.
    pub(crate) error: String,
}
