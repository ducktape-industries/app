//! IANA special-purpose address rows and longest-prefix classification.

use std::net::{Ipv4Addr, Ipv6Addr};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GloballyReachable {
    Yes,
    No,
}

pub(crate) const IANA_IPV4_SPECIAL_PURPOSE_ADDRESS_REGISTRY: &[(
    Ipv4Addr,
    u32,
    GloballyReachable,
)] = &[
    (Ipv4Addr::new(0, 0, 0, 0), 8, GloballyReachable::No),
    (Ipv4Addr::new(10, 0, 0, 0), 8, GloballyReachable::No),
    (Ipv4Addr::new(100, 64, 0, 0), 10, GloballyReachable::No),
    (Ipv4Addr::new(127, 0, 0, 0), 8, GloballyReachable::No),
    (Ipv4Addr::new(169, 254, 0, 0), 16, GloballyReachable::No),
    (Ipv4Addr::new(172, 16, 0, 0), 12, GloballyReachable::No),
    (Ipv4Addr::new(192, 0, 0, 0), 24, GloballyReachable::No),
    (Ipv4Addr::new(192, 0, 0, 9), 32, GloballyReachable::Yes),
    (Ipv4Addr::new(192, 0, 0, 10), 32, GloballyReachable::Yes),
    (Ipv4Addr::new(192, 0, 2, 0), 24, GloballyReachable::No),
    (Ipv4Addr::new(192, 88, 99, 2), 32, GloballyReachable::No),
    (Ipv4Addr::new(192, 168, 0, 0), 16, GloballyReachable::No),
    (Ipv4Addr::new(198, 18, 0, 0), 15, GloballyReachable::No),
    (Ipv4Addr::new(198, 51, 100, 0), 24, GloballyReachable::No),
    (Ipv4Addr::new(203, 0, 113, 0), 24, GloballyReachable::No),
    (Ipv4Addr::new(240, 0, 0, 0), 4, GloballyReachable::No),
    (Ipv4Addr::new(255, 255, 255, 255), 32, GloballyReachable::No),
];

pub(crate) const IANA_IPV6_SPECIAL_PURPOSE_ADDRESS_REGISTRY: &[(
    Ipv6Addr,
    u32,
    GloballyReachable,
)] = &[
    (
        Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0),
        128,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1),
        128,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0),
        48,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0),
        64,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x100, 0, 0, 1, 0, 0, 0, 0),
        64,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0),
        23,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x2001, 1, 0, 0, 0, 0, 0, 1),
        128,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 1, 0, 0, 0, 0, 0, 2),
        128,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 1, 0, 0, 0, 0, 0, 3),
        128,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 3, 0, 0, 0, 0, 0, 0),
        32,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 4, 0x112, 0, 0, 0, 0, 0),
        48,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 0x20, 0, 0, 0, 0, 0, 0),
        28,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 0x30, 0, 0, 0, 0, 0, 0),
        28,
        GloballyReachable::Yes,
    ),
    (
        Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0),
        32,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0),
        20,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0x5f00, 0, 0, 0, 0, 0, 0, 0),
        16,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0),
        7,
        GloballyReachable::No,
    ),
    (
        Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0),
        10,
        GloballyReachable::No,
    ),
];

pub(crate) const RFC_6052_WELL_KNOWN_PREFIX: Ipv6Addr =
    Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0);
pub(crate) const IPV4_MAPPED_PREFIX: Ipv6Addr = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0, 0);

pub(crate) fn reachable_v4(ip: Ipv4Addr) -> bool {
    let bits = u32::from(ip);
    IANA_IPV4_SPECIAL_PURPOSE_ADDRESS_REGISTRY
        .iter()
        .filter(|(net, len, _)| bits >> (32 - len) == u32::from(*net) >> (32 - len))
        .max_by_key(|(_, len, _)| *len)
        .is_none_or(|(_, _, reachable)| *reachable == GloballyReachable::Yes)
}

pub(crate) fn reachable_v6(ip: Ipv6Addr) -> bool {
    if let Some(ip) = ip.to_ipv4_mapped() {
        return reachable_v4(ip);
    }
    let bits = u128::from(ip);
    if bits >> 32 == u128::from(RFC_6052_WELL_KNOWN_PREFIX) >> 32 {
        return reachable_v4(Ipv4Addr::from(bits as u32));
    }
    IANA_IPV6_SPECIAL_PURPOSE_ADDRESS_REGISTRY
        .iter()
        .filter(|(net, len, _)| bits >> (128 - len) == u128::from(*net) >> (128 - len))
        .max_by_key(|(_, len, _)| *len)
        .is_none_or(|(_, _, reachable)| *reachable == GloballyReachable::Yes)
}
