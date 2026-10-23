//! URL filters generated from the shared registries for WKContentRuleListStore.

use super::registry::{
    GloballyReachable, IANA_IPV4_SPECIAL_PURPOSE_ADDRESS_REGISTRY,
    IANA_IPV6_SPECIAL_PURPOSE_ADDRESS_REGISTRY, IPV4_MAPPED_PREFIX, RFC_6052_WELL_KNOWN_PREFIX,
};
use std::net::{Ipv4Addr, Ipv6Addr};

#[derive(Clone, Copy)]
struct Block {
    network: u128,
    prefix: u32,
}

pub fn webkit_content_rules() -> String {
    let v4_rows: Vec<_> = IANA_IPV4_SPECIAL_PURPOSE_ADDRESS_REGISTRY
        .iter()
        .map(|(ip, prefix, reachable)| (u128::from(u32::from(*ip)), *prefix, *reachable))
        .collect();
    let v6_rows: Vec<_> = IANA_IPV6_SPECIAL_PURPOSE_ADDRESS_REGISTRY
        .iter()
        .map(|(ip, prefix, reachable)| (u128::from(*ip), *prefix, *reachable))
        .collect();
    let v4_blocks = denied_blocks(&v4_rows, u32::BITS);
    let mut v6_blocks = denied_blocks(&v6_rows, u128::BITS);
    for prefix in [IPV4_MAPPED_PREFIX, RFC_6052_WELL_KNOWN_PREFIX] {
        v6_blocks.extend(v4_blocks.iter().map(|block| Block {
            network: u128::from(prefix) | block.network,
            prefix: u128::BITS - u32::BITS + block.prefix,
        }));
    }
    let mut hosts = vec![r"([^/:]*\.)?localhost\.*".to_owned()];
    hosts.extend(v4_blocks.into_iter().flat_map(ipv4_hosts));
    hosts.extend(v6_blocks.into_iter().flat_map(ipv6_hosts));
    let rules = hosts
        .into_iter()
        .flat_map(|host| {
            ["https?", "wss?"].map(move |scheme| {
                serde_json::json!({
                    "trigger": {
                        "url-filter": format!("^{scheme}://([^/]*@)?{host}(:[0-9]+)?/")
                    },
                    "action": { "type": "block" }
                })
            })
        })
        .collect();
    serde_json::Value::Array(rules).to_string()
}

fn denied_blocks(rows: &[(u128, u32, GloballyReachable)], width: u32) -> Vec<Block> {
    rows.iter()
        .filter(|(_, _, reachable)| *reachable == GloballyReachable::No)
        .flat_map(|&(network, prefix, _)| {
            let mut blocks = vec![Block { network, prefix }];
            for &(other_network, other_prefix, _) in rows {
                if other_prefix <= prefix {
                    continue;
                }
                let removed = Block {
                    network: other_network,
                    prefix: other_prefix,
                };
                blocks = blocks
                    .into_iter()
                    .flat_map(|block| subtract(block, removed, width))
                    .collect();
            }
            blocks
        })
        .collect()
}

fn subtract(block: Block, removed: Block, width: u32) -> Vec<Block> {
    let shared = block.prefix.min(removed.prefix);
    let overlaps = (block.network ^ removed.network) >> (width - shared) == 0;
    if !overlaps {
        return vec![block];
    }
    if removed.prefix <= block.prefix {
        return Vec::new();
    }
    let prefix = block.prefix + 1;
    let half = 1_u128 << (width - prefix);
    [block.network, block.network | half]
        .into_iter()
        .flat_map(|network| subtract(Block { network, prefix }, removed, width))
        .collect()
}

fn ipv4_hosts(block: Block) -> Vec<String> {
    let octets = Ipv4Addr::from(block.network as u32).octets();
    let full = (block.prefix / u8::BITS) as usize;
    let partial = block.prefix % u8::BITS;
    let mut patterns = vec![octets[..full].iter().map(u8::to_string).collect::<Vec<_>>()];
    if partial != 0 {
        patterns = (octets[full]..=octets[full] + (u8::MAX >> partial))
            .map(|octet| {
                let mut parts: Vec<_> = octets[..full].iter().map(u8::to_string).collect();
                parts.push(octet.to_string());
                parts
            })
            .collect();
    }
    patterns
        .into_iter()
        .map(|mut parts| {
            parts.resize_with(octets.len(), || "[0-9]+".to_owned());
            parts.join(r"\.")
        })
        .collect()
}

fn ipv6_hosts(block: Block) -> Vec<String> {
    let segments = Ipv6Addr::from(block.network).segments();
    let choices: Vec<_> = segments
        .iter()
        .enumerate()
        .map(|(index, &segment)| {
            let prefix = block.prefix.saturating_sub(index as u32 * u16::BITS);
            hextet_patterns(segment, prefix.min(u16::BITS))
        })
        .collect();
    let mut hosts = joined_segments(&choices);
    for start in 0..segments.len() {
        for end in start + 1..=segments.len() {
            let may_compress = segments[start..end].iter().all(|&segment| segment == 0);
            if !may_compress {
                continue;
            }
            for before in joined_segments(&choices[..start]) {
                for after in joined_segments(&choices[end..]) {
                    hosts.push(format!("{before}::{after}"));
                }
            }
        }
    }
    hosts
        .into_iter()
        .map(|host| format!(r"\[{host}\]"))
        .collect()
}

fn joined_segments(choices: &[Vec<String>]) -> Vec<String> {
    choices.iter().fold(vec![String::new()], |joined, choices| {
        joined
            .into_iter()
            .flat_map(|prefix| {
                choices.iter().map(move |segment| {
                    if prefix.is_empty() {
                        return segment.clone();
                    }
                    format!("{prefix}:{segment}")
                })
            })
            .collect()
    })
}

fn hextet_patterns(network: u16, prefix: u32) -> Vec<String> {
    match prefix {
        0 => return vec!["[0-9a-f]+".to_owned()],
        u16::BITS => {
            let pattern = match network {
                0 => "0+".to_owned(),
                _ => format!("0*{network:x}"),
            };
            return vec![pattern];
        }
        _ => {}
    }
    let bits_per_digit = 4;
    let digits_per_segment = u16::BITS / bits_per_digit;
    let digit_max = (1_u16 << bits_per_digit) - 1;
    let mut patterns = Vec::new();
    for digits in 1..=digits_per_segment {
        let discarded_bits = u32::from(network) >> (digits * bits_per_digit);
        if discarded_bits != 0 {
            continue;
        }
        let mut pattern = "0*".to_owned();
        for digit in (0..digits).rev() {
            let shift = digit * bits_per_digit;
            let fixed_bits = prefix
                .saturating_sub(u16::BITS - shift - bits_per_digit)
                .min(bits_per_digit);
            let low = (network >> shift) & digit_max;
            let high = low | (digit_max >> fixed_bits);
            let is_leading_digit = digit == digits - 1 && digits > 1;
            let low = if is_leading_digit { low.max(1) } else { low };
            if low > high {
                pattern.clear();
                break;
            }
            match low == high {
                true => pattern.push_str(&format!("{low:x}")),
                false => {
                    pattern.push('[');
                    for value in low..=high {
                        pattern.push_str(&format!("{value:x}"));
                    }
                    pattern.push(']');
                }
            }
        }
        if !pattern.is_empty() {
            patterns.push(pattern);
        }
    }
    patterns
}
