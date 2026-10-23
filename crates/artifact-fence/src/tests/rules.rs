use super::*;
use crate::registry::{
    GloballyReachable, IANA_IPV4_SPECIAL_PURPOSE_ADDRESS_REGISTRY,
    IANA_IPV6_SPECIAL_PURPOSE_ADDRESS_REGISTRY, IPV4_MAPPED_PREFIX, RFC_6052_WELL_KNOWN_PREFIX,
};
use regex::Regex;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

fn filters() -> &'static [Regex] {
    static FILTERS: OnceLock<Vec<Regex>> = OnceLock::new();
    FILTERS.get_or_init(|| {
        let rules: serde_json::Value = serde_json::from_str(&webkit_content_rules()).unwrap();
        let rules = rules.as_array().unwrap();
        assert!(!rules.is_empty());
        rules
            .iter()
            .map(|rule| {
                assert_eq!(rule["action"], serde_json::json!({ "type": "block" }));
                let trigger = rule["trigger"].as_object().unwrap();
                let pattern = trigger["url-filter"].as_str().unwrap();
                assert_eq!(trigger.keys().collect::<Vec<_>>(), vec!["url-filter"]);
                assert!(pattern.starts_with('^'), "{pattern}");
                for unsupported in ['|', '{', '}', '$'] {
                    assert!(!pattern.contains(unsupported), "{pattern}");
                }
                assert!(!pattern.contains("(?"), "{pattern}");
                Regex::new(pattern).unwrap()
            })
            .collect()
    })
}

fn blocked(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    filters().iter().any(|filter| filter.is_match(url.as_str()))
}

fn check(address: &str) {
    assert_eq!(blocked(address), private(address), "{address}");
}

fn check_v4(bits: u32) {
    let ip = Ipv4Addr::from(bits);
    for scheme in ["http", "https", "ws", "wss"] {
        check(&format!("{scheme}://{ip}/"));
        for prefix in [IPV4_MAPPED_PREFIX, RFC_6052_WELL_KNOWN_PREFIX] {
            let ip = Ipv6Addr::from(u128::from(prefix) | u128::from(bits));
            check(&format!("{scheme}://[{ip}]/"));
        }
    }
}

fn check_v6(bits: u128) {
    let ip = Ipv6Addr::from(bits);
    for scheme in ["http", "https", "ws", "wss"] {
        check(&format!("{scheme}://[{ip}]/"));
    }
}

#[test]
fn every_ipv4_row_matches_its_edges_and_neighbors_in_generated_rules() {
    for &(network, prefix, reachable) in IANA_IPV4_SPECIAL_PURPOSE_ADDRESS_REGISTRY {
        let first = u32::from(network);
        let last = first | u32::MAX.checked_shr(prefix).unwrap_or(0);
        for edge in [first, last] {
            let address = format!("https://{}/", Ipv4Addr::from(edge));
            assert_eq!(private(&address), reachable == GloballyReachable::No);
            check_v4(edge);
        }
        if let Some(before) = first.checked_sub(1) {
            check_v4(before);
        }
        if let Some(after) = last.checked_add(1) {
            check_v4(after);
        }
        let interior = first | (u32::MAX.checked_shr(prefix).unwrap_or(0) & 0x5555_5555);
        check_v4(interior);
    }
}

#[test]
fn every_ipv6_row_matches_its_edges_and_neighbors_in_generated_rules() {
    for &(network, prefix, reachable) in IANA_IPV6_SPECIAL_PURPOSE_ADDRESS_REGISTRY {
        let first = u128::from(network);
        let last = first | u128::MAX.checked_shr(prefix).unwrap_or(0);
        for edge in [first, last] {
            let address = format!("https://[{}]/", Ipv6Addr::from(edge));
            assert_eq!(private(&address), reachable == GloballyReachable::No);
            check_v6(edge);
        }
        if let Some(before) = first.checked_sub(1) {
            check_v6(before);
        }
        if let Some(after) = last.checked_add(1) {
            check_v6(after);
        }
        let interior =
            first | (u128::MAX.checked_shr(prefix).unwrap_or(0) & u128::from_ne_bytes([0x55; 16]));
        check_v6(interior);
    }
}

#[test]
fn literal_rules_match_hosts_not_usernames_paths_or_neighboring_names() {
    for (address, expected) in [
        ("https://user:secret@127.0.0.1:8443/page?x=1", true),
        ("https://user:secret@[::1]:8443/page", true),
        ("http://LOCALHOST./", true),
        ("http://a.b.localhost.../", true),
        ("ws://a_b.localhost:9222/?x=1", true),
        ("http://0x7f000001/", true),
        ("http://0177.0.0.1/", true),
        ("http://2130706433/?x=1", true),
        ("http://127.1/", true),
        ("https://[::ffff:127.0.0.1]/", true),
        ("https://[64:ff9b::192.168.0.1]/", true),
        ("https://[0000:0000:0000:0000:0000:ffff:7f00:0001]/", true),
        ("https://127.0.0.1@example.com/", false),
        ("https://localhost@example.com/", false),
        ("https://example.com/127.0.0.1/", false),
        ("https://example.com/?next=http://localhost/", false),
        ("https://localhost.example.com/", false),
        ("https://notlocalhost/", false),
        ("http://127.0.0.1.example.com/", false),
        ("duck://localhost/files/page.html", false),
        ("ftp://127.0.0.1/", false),
        ("data:text/plain,http://127.0.0.1/", false),
        ("https://[::ffff:192.0.0.9]/", false),
        ("https://[64:ff9b::192.0.0.10]/", false),
        ("https://[2001:1::1]/", false),
        ("https://[2001:3:ffff:ffff:ffff:ffff:ffff:ffff]/", false),
        ("https://[2001:4:112::1]/", false),
        ("https://[2001:20::1]/", false),
        ("https://[2001:30::1]/", false),
    ] {
        assert_eq!(private(address), expected, "{address}");
        assert_eq!(blocked(address), expected, "{address}");
    }
}

#[test]
fn ipv6_rules_accept_uncompressed_and_each_valid_zero_compression() {
    for &(network, prefix, _) in IANA_IPV6_SPECIAL_PURPOSE_ADDRESS_REGISTRY {
        let last = Ipv6Addr::from(u128::from(network) | u128::MAX.checked_shr(prefix).unwrap_or(0));
        for ip in [network, last] {
            let parts: Vec<_> = ip
                .segments()
                .iter()
                .map(|part| format!("{part:04x}"))
                .collect();
            let expanded = format!("https://[{}]/", parts.join(":"));
            let expected = private(&expanded);
            let matches = filters().iter().any(|filter| filter.is_match(&expanded));
            assert_eq!(matches, expected, "{expanded}");
            for start in 0..parts.len() {
                for end in start + 1..=parts.len() {
                    let all_zero = ip.segments()[start..end].iter().all(|part| *part == 0);
                    if !all_zero {
                        continue;
                    }
                    let address = format!(
                        "https://[{}::{}]/",
                        parts[..start].join(":"),
                        parts[end..].join(":")
                    );
                    let matches = filters().iter().any(|filter| filter.is_match(&address));
                    assert_eq!(matches, expected, "{address}");
                }
            }
        }
    }
}
