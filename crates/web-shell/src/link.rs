//! The page's path read as a duck link, `/<chain>/<program>/<route…>`: the program it opens and the route handed to its view;
//! and what a link a view opens does: a duck link loads its path, an http or https link opens a new tab, and nothing else opens.

#[cfg_attr(
    not(target_family = "wasm"),
    allow(dead_code, reason = "the page reads it; its test runs natively")
)]
const FIRST_PROGRAM: &str = "chat";

#[cfg_attr(
    not(target_family = "wasm"),
    allow(dead_code, reason = "the page reads it; its test runs natively")
)]
#[derive(Debug, PartialEq, Eq)]
pub struct Opened {
    pub program: String,
    pub route: Option<String>,
}

#[cfg_attr(
    not(target_family = "wasm"),
    allow(dead_code, reason = "the page reads it; its test runs natively")
)]
pub fn from_path(path: &str) -> Opened {
    let mut parts = path.trim_matches('/').splitn(3, '/');
    let _chain = parts.next();
    let program = parts
        .next()
        .filter(|program| !program.is_empty())
        .unwrap_or(FIRST_PROGRAM)
        .to_owned();
    let route = parts
        .next()
        .map(|route| route.trim_end_matches('/'))
        .filter(|route| !route.is_empty())
        .map(str::to_owned);
    Opened { program, route }
}

#[cfg_attr(
    not(target_family = "wasm"),
    allow(dead_code, reason = "the page reads it; its test runs natively")
)]
#[derive(Debug, PartialEq, Eq)]
pub enum Opening<'a> {
    Duck(&'a str),
    Web,
    Refused,
}

#[cfg_attr(
    not(target_family = "wasm"),
    allow(dead_code, reason = "the page reads it; its test runs natively")
)]
pub fn opening(link: &str) -> Opening<'_> {
    let scheme_is = |scheme: &str| {
        link.get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    };
    if scheme_is("duck://") {
        return Opening::Duck(&link["duck://".len()..]);
    }
    let is_web_page = scheme_is("https://") || scheme_is("http://");
    match is_web_page {
        true => Opening::Web,
        false => Opening::Refused,
    }
}

#[cfg(target_family = "wasm")]
pub fn open(link: &str) {
    let page = crate::node::page();
    let opened = match opening(link) {
        Opening::Duck(path) => page.location().set_pathname(&format!("/{path}")),
        Opening::Web => page
            .open_with_url_and_target_and_features(link, "_blank", "noopener")
            .map(drop),
        Opening::Refused => Err(wasm_bindgen::JsValue::from_str(
            "a view's link opens only as a duck, http or https link",
        )),
    };
    if let Err(refused) = opened {
        web_sys::console::error_2(&format!("the page could not open {link}").into(), &refused);
    }
}

#[cfg(test)]
mod tests {
    use super::{Opened, Opening, from_path, opening};

    fn opened(program: &str, route: Option<&str>) -> Opened {
        Opened {
            program: program.to_owned(),
            route: route.map(str::to_owned),
        }
    }

    #[test]
    fn the_root_opens_the_first_program_with_no_route() {
        assert_eq!(from_path("/"), opened("chat", None));
        assert_eq!(from_path(""), opened("chat", None));
        assert_eq!(from_path("/duckpond-725bcad1"), opened("chat", None));
        assert_eq!(from_path("/duckpond-725bcad1/"), opened("chat", None));
    }

    #[test]
    fn a_program_and_its_route_come_from_the_path_after_the_chain() {
        assert_eq!(from_path("/duckpond-725bcad1/forge"), opened("forge", None));
        assert_eq!(
            from_path("/duckpond-725bcad1/chat/channel-18df5a416a71b5fa-1792250406982472"),
            opened("chat", Some("channel-18df5a416a71b5fa-1792250406982472"))
        );
        assert_eq!(
            from_path("/duckpond-725bcad1/forge/app/changes/221/"),
            opened("forge", Some("app/changes/221"))
        );
    }

    #[test]
    fn a_duck_link_opens_its_path_and_a_web_link_opens_a_tab() {
        assert_eq!(
            opening("duck://duckpond-725bcad1/forge/app"),
            Opening::Duck("duckpond-725bcad1/forge/app")
        );
        assert_eq!(opening("https://ducktape.industries"), Opening::Web);
        assert_eq!(opening("http://100.65.253.102:8846/"), Opening::Web);
        assert_eq!(opening("HTTPS://ducktape.industries"), Opening::Web);
    }

    #[test]
    fn a_link_of_any_other_scheme_opens_nothing() {
        for link in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "JAVASCRIPT:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox(1)",
            "file:///etc/passwd",
            " https://ducktape.industries",
            "\tjavascript:alert(1)",
            "//ducktape.industries",
            "https:",
            "",
        ] {
            assert_eq!(opening(link), Opening::Refused, "{link:?}");
        }
    }
}
