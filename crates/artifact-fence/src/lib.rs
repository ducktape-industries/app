//! Shared artifact address checks and WebKit content-rule JSON for the desktop and phone.

mod registry;
mod webkit;

pub use webkit::webkit_content_rules;

pub const CONTENT_SECURITY_POLICY: &str = "default-src duck: data: blob: 'unsafe-inline'; script-src duck: data: blob: 'unsafe-inline' 'unsafe-eval' https://cdnjs.cloudflare.com; worker-src duck: data: blob:; object-src 'none'";

pub fn private(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.');
            name == "localhost" || name.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => !registry::reachable_v4(ip),
        Some(url::Host::Ipv6(ip)) => !registry::reachable_v6(ip),
        None => false,
    }
}

#[cfg(test)]
mod tests;
