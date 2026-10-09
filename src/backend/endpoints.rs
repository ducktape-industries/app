//! Node addresses: what an endpoint is (an http(s) origin, nothing more),
//! and the eight this device connected to most recently, kept in
//! prefs.json with the network each reported.

use super::prefs::{edit_prefs, read_prefs};

pub(crate) const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8844";

pub(crate) const ENDPOINT_REFUSAL: &str = "A node address is a host and an optional port (127.0.0.1:8844), with http:// or https:// in front if you like, and nothing else.";

/// The origin of a node URL, or `None` for anything that is not one. A bare
/// `host:port` is what a node prints and what people paste, so no scheme
/// means `http://` — checked by `://`, since `localhost:8844` would
/// otherwise parse as a URL whose scheme is `localhost`.
pub(crate) fn endpoint_origin(url: &str) -> Option<String> {
    let url = url.trim();
    let url = match url.contains("://") {
        true => reqwest::Url::parse(url).ok()?,
        false => reqwest::Url::parse(&format!("http://{url}")).ok()?,
    };
    let origin = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "" | "/");
    origin.then(|| url.as_str().trim_end_matches('/').to_string())
}

/// A node URL this device connected to, and the network name it reported.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RecentEndpoint {
    pub(crate) url: String,
    pub(crate) network: String,
    /// The network's founding time, which tells two chains with one name
    /// apart.
    pub(crate) founded: u64,
    /// This node's chain is not the one this device first met under its
    /// name ([`Keyring::other_chain`](super::key_dir::Keyring::other_chain)).
    pub(crate) other_chain: bool,
}

/// A node URL's host and port — the URL without its scheme.
pub(crate) fn host_of(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, host)| host)
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

    #[test]
    fn an_endpoint_is_an_origin_and_nothing_else() {
        assert_eq!(
            endpoint_origin(" http://127.0.0.1:8844/ ").as_deref(),
            Some("http://127.0.0.1:8844")
        );
        assert_eq!(endpoint_origin("http://a:b@host"), None);
        assert_eq!(endpoint_origin("http://host/v1"), None);
        assert_eq!(endpoint_origin("ftp://host"), None);
        assert_eq!(
            endpoint_origin("127.0.0.1:34329").as_deref(),
            Some("http://127.0.0.1:34329")
        );
        assert_eq!(
            endpoint_origin(" localhost:8844 ").as_deref(),
            Some("http://localhost:8844")
        );
        for junk in [
            "",
            "hello world",
            "host:port",
            "host/v1",
            "a:b@host",
            "host?x=1",
        ] {
            assert_eq!(endpoint_origin(junk), None, "{junk:?}");
        }
    }

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
