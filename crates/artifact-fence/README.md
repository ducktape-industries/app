# Artifact address fence

`private(address)` checks an HTTP(S) or WebSocket URL with the WHATWG parser.
It refuses localhost and its subdomains, and literal IP addresses the IANA
special-purpose registries mark not globally reachable. The most specific
entry wins. IPv4-mapped IPv6 and the RFC 6052 well-known prefix use the IPv4
classification. Other schemes and DNS names are not refused by this check.

The tables in `registry.rs` are shared by the desktop web view and the phone.
Their source is IANA's [IPv4 special-purpose registry](https://www.iana.org/assignments/iana-ipv4-special-registry/iana-ipv4-special-registry.xhtml)
and [IPv6 special-purpose registry](https://www.iana.org/assignments/iana-ipv6-special-registry/iana-ipv6-special-registry.xhtml).

`webkit_content_rules()` returns the JSON array a `WKContentRuleListStore`
compiles. Its block actions cover the same literal hosts and localhost over
HTTP(S) and WebSocket URLs. More-specific table rows are subtracted before
rules are generated, so public exceptions need no `ignore-previous-rules`
actions. Filters use character classes, groups and `?`, `*`, `+` quantifiers,
not alternation or lookaround. IPv6 filters cover zero compression and
leading zeros; numeric IPv4 spellings are matched after URL canonicalization.

These rules do not resolve DNS names, block WebRTC or supply a page's CSP.
The caller must install the compiled list on its artifact web view. The
crate itself opens no web view and starts no network work.

`CONTENT_SECURITY_POLICY` is the artifact response header shared with the
desktop's duck-scheme handler. It allows page resources from `duck:`, `data:`
and `blob:`, and scripts additionally from cdnjs; objects are refused.

Tests read the generated JSON, compile its URL filters with Rust's regex
engine and compare their matches with the shared address check at every
registry row's edges and neighboring addresses. This is not a WebKit
compiler or an iOS simulator check.
