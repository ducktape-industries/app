//! Node addresses: what an endpoint is (an http(s) origin, nothing more).

pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8844";

pub const ENDPOINT_REFUSAL: &str = "A node address is a host and an optional port (127.0.0.1:8844), with http:// or https:// in front if you like, and nothing else.";

/// The origin of a node URL, or `None` for anything that is not one. A bare
/// `host:port` is what a node prints and what people paste, so no scheme
/// means `http://` — checked by `://`, since `localhost:8844` would
/// otherwise parse as a URL whose scheme is `localhost`.
pub fn endpoint_origin(url: &str) -> Option<String> {
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

/// A node URL's host and port — the URL without its scheme.
pub fn host_of(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, host)| host)
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
}
