//! Sentences for the person, off errors the transport produced.

/// A sentence for a connection attempt that did not reach a node at `host`,
/// for the common shapes of `message`: nothing answered (reqwest's "error
/// sending request for url (...)"), something answered but not as a node
/// ([`crate::NOT_A_NODE`]), the connect step's own timeout; `None` for any
/// other. `device` is what the person calls this device ("phone",
/// "computer"): the network it is on is this device's.
pub fn unreachable(host: &str, message: &str, device: &str) -> Option<String> {
    if message.contains("error sending request") {
        return Some(format!(
            "Nothing answered at {host}. Check the address, and that the node is up and \
             reachable from this {device}'s network."
        ));
    }
    if message.contains(crate::NOT_A_NODE) {
        return Some(format!(
            "Something answered at {host}, but not a ducktape node. Check the address."
        ));
    }
    if message.contains("did not answer in time") {
        return Some(format!(
            "{host} has not answered. Check that the node is running, then Connect again."
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transport_failure_names_the_node_not_the_raw_error() {
        let sentence = unreachable(
            "127.0.0.1:33835",
            "error sending request for url (http://127.0.0.1:33835/v1/status)",
            "phone",
        );
        assert_eq!(
            sentence.as_deref(),
            Some(
                "Nothing answered at 127.0.0.1:33835. Check the address, and that the node is up \
                 and reachable from this phone's network."
            )
        );
        let other = unreachable(
            "example.com",
            &format!("node answered 301: {}", crate::NOT_A_NODE),
            "computer",
        );
        assert_eq!(
            other.as_deref(),
            Some("Something answered at example.com, but not a ducktape node. Check the address.")
        );
    }
}
