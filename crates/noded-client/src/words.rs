//! Sentences for the person, off errors the transport produced.

/// A sentence for a connection attempt that never reached `origin`, for the
/// common shapes of `message` — reqwest's "error sending request for url
/// (...)", the connect step's own timeout; `None` for any other.
pub fn unreachable(origin: &str, message: &str) -> Option<String> {
    if message.contains("error sending request") {
        return Some(format!(
            "Can't reach {origin}. Check the address, or that the node is running."
        ));
    }
    if message.contains("did not answer in time") {
        return Some(format!(
            "{origin} has not answered. Check that the node is running, then Connect again."
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
            "http://127.0.0.1:33835",
            "error sending request for url (http://127.0.0.1:33835/v1/status)",
        );
        assert_eq!(
            sentence.as_deref(),
            Some(
                "Can't reach http://127.0.0.1:33835. Check the address, or that the node is running."
            )
        );
    }
}
