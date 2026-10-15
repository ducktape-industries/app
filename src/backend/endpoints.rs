//! The eight node addresses this device connected to most recently, kept
//! in prefs.json with the network each reported. What an address is:
//! `noded_client::endpoint`.

use noded_client::endpoint::host_of;

use super::prefs::{edit_prefs, read_prefs};

/// A node URL this device connected to, and the network name it reported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RecentEndpoint {
    pub(crate) url: String,
    pub(crate) network: String,
    /// The network's founding time, which tells two chains with one name
    /// apart.
    pub(crate) founded: u64,
    /// This node's chain is not the one this device first met under its
    /// name ([`Keyring::other_chain`](noded_client::keyring::Keyring::other_chain)).
    pub(crate) other_chain: bool,
}

impl RecentEndpoint {
    /// The node's host and port — the URL without its scheme.
    pub(crate) fn host(&self) -> &str {
        host_of(&self.url)
    }

    /// The network's name.
    pub(crate) fn name(&self) -> String {
        self.network.clone()
    }

    /// How a list names this node: its network and host, and — when two
    /// chains share the name — which one is the other chain. Two rows
    /// never read the same: the list holds one row per URL.
    pub(crate) fn label(&self) -> String {
        match self.other_chain {
            false => format!("{} · {}", self.network, self.host()),
            true => format!("{} · {} · different network", self.network, self.host()),
        }
    }
}

/// Node URLs this device connected to, most recent first. An entry that
/// does not read as `{"url", "network", ..}` is dropped; the rest stand.
pub(crate) fn recent_endpoints() -> Vec<RecentEndpoint> {
    endpoints_of(&read_prefs().unwrap_or_default())
}

fn endpoints_of(prefs: &serde_json::Value) -> Vec<RecentEndpoint> {
    prefs["endpoints"]
        .as_array()
        .map(|list| list.iter().filter_map(endpoint_of_json).collect())
        .unwrap_or_default()
}

fn endpoint_of_json(value: &serde_json::Value) -> Option<RecentEndpoint> {
    Some(RecentEndpoint {
        url: value.get("url")?.as_str()?.to_owned(),
        network: value.get("network")?.as_str()?.to_owned(),
        founded: value["founded"].as_u64().unwrap_or_default(),
        other_chain: value["other_chain"].as_bool().unwrap_or_default(),
    })
}

/// Moves `entry` to the front: its URL, with the network it just reported.
pub(crate) fn note_endpoint(entry: RecentEndpoint) {
    edit_endpoints(|recent| note(recent, entry));
}

fn note(recent: &mut Vec<RecentEndpoint>, entry: RecentEndpoint) {
    recent.retain(|known| known.url != entry.url);
    recent.insert(0, entry);
    recent.truncate(8);
}

/// Drops `endpoint` from the recent list — the row's Forget button.
pub(crate) fn forget_endpoint(endpoint: &str) {
    edit_endpoints(|recent| recent.retain(|known| known.url != endpoint));
}

/// The list read, changed and written back in one prefs edit.
fn edit_endpoints(change: impl FnOnce(&mut Vec<RecentEndpoint>)) {
    edit_prefs(|prefs| {
        let mut recent = endpoints_of(prefs);
        change(&mut recent);
        prefs["endpoints"] = serde_json::Value::Array(entries_of(&recent));
    });
}

fn entries_of(recent: &[RecentEndpoint]) -> Vec<serde_json::Value> {
    recent
        .iter()
        .map(|entry| {
            serde_json::json!({
                "url": entry.url,
                "network": entry.network,
                "founded": entry.founded,
                "other_chain": entry.other_chain,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prefs file from before this shape — bare strings, entries without
    /// a network, junk — loses those entries and nothing else.
    #[test]
    fn an_unreadable_recent_entry_is_dropped_and_the_rest_kept() {
        let list = serde_json::json!([
            "http://a",
            {"url": "http://b"},
            7,
            {"network": "testkit"},
            {"url": "http://c", "network": "testkit"},
        ]);
        let parsed: Vec<_> = list
            .as_array()
            .unwrap()
            .iter()
            .filter_map(endpoint_of_json)
            .collect();
        assert_eq!(
            parsed,
            [RecentEndpoint {
                url: "http://c".into(),
                network: "testkit".into(),
                ..RecentEndpoint::default()
            }]
        );
    }

    #[test]
    fn noting_an_endpoint_moves_it_to_the_front_and_dedupes() {
        let entry = |url: &str, network: &str| RecentEndpoint {
            url: url.into(),
            network: network.into(),
            ..RecentEndpoint::default()
        };
        let mut recent = vec![entry("http://a", "x")];
        note(&mut recent, entry("http://b", "testkit"));
        note(&mut recent, entry("http://a", "renamed"));
        assert_eq!(
            recent.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
            ["http://a", "http://b"]
        );
        assert_eq!(recent[0].network, "renamed");
    }

    #[test]
    fn recent_rows_name_the_host_and_mark_the_other_chain() {
        let row = |url: &str, other_chain| RecentEndpoint {
            url: url.into(),
            network: "testkit".into(),
            founded: 1,
            other_chain,
        };
        assert_eq!(
            row("http://127.0.0.1:36817", false).label(),
            "testkit · 127.0.0.1:36817"
        );
        assert_eq!(
            row("http://127.0.0.1:34329", true).label(),
            "testkit · 127.0.0.1:34329 · different network"
        );
        let saved = serde_json::json!({"url": "http://c", "network": "testkit", "founded": 9, "other_chain": true});
        assert_eq!(endpoint_of_json(&saved).unwrap(), {
            let mut c = row("http://c", true);
            c.founded = 9;
            c
        });
    }
}
