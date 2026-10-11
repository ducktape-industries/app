//! Where peers dial this node (`found --address`, `join --address`): a transport, a host and a
//! port. It decides which sockets the node listens on and where a peer joins through. A copy of
//! qa's kit `crates/kit/src/address.rs`; here a host also refuses `"` and `\`, as it is written
//! into the founding file.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Tls,
    Ws,
    Wss,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub transport: Transport,
    pub host: String,
    pub port: u16,
}

const FORMS: &str = "tcp://host:port, tls://host:port, ws://host[:port] or wss://host[:port]";

impl Transport {
    fn name(self) -> &'static str {
        match self {
            Transport::Tcp => "tcp",
            Transport::Tls => "tls",
            Transport::Ws => "ws",
            Transport::Wss => "wss",
        }
    }

    fn default_port(self) -> Option<u16> {
        match self {
            Transport::Tcp | Transport::Tls => None,
            Transport::Ws => Some(80),
            Transport::Wss => Some(443),
        }
    }
}

impl FromStr for Address {
    type Err = String;

    fn from_str(text: &str) -> Result<Address, String> {
        let Some((scheme, rest)) = text.split_once("://") else {
            return Err(format!("`{text}` names no transport: write {FORMS}"));
        };
        let transport = match scheme {
            "tcp" => Transport::Tcp,
            "tls" => Transport::Tls,
            "ws" => Transport::Ws,
            "wss" => Transport::Wss,
            other => return Err(format!("`{other}` is not a transport: write {FORMS}")),
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        let (host, port) = host_and_port(rest).map_err(|why| format!("`{text}`: {why}"))?;
        let stray = host.is_empty() || host.contains(['/', '@', '?', '#', ' ', '"', '\\']);
        if stray {
            return Err(format!("`{text}`: write {FORMS}"));
        }
        let Some(port) = port.or(transport.default_port()) else {
            return Err(format!("`{text}` names no port"));
        };
        Ok(Address {
            transport,
            host: host.to_string(),
            port,
        })
    }
}

fn host_and_port(rest: &str) -> Result<(&str, Option<u16>), String> {
    let (host, port) = match rest.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']').ok_or("an unclosed `[`")?;
            let port = match after {
                "" => None,
                after => Some(after.strip_prefix(':').ok_or("junk after `]`")?),
            };
            (host, port)
        }
        None => match rest.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        },
    };
    let port = port
        .map(|port| {
            port.parse::<u16>()
                .map_err(|_| format!("`{port}` is not a port"))
        })
        .transpose()?;
    Ok((host, port))
}

impl Address {
    fn authority(&self, port: u16) -> String {
        let bracketed = self.host.contains(':');
        if bracketed {
            return format!("[{}]:{port}", self.host);
        }
        format!("{}:{port}", self.host)
    }

    pub fn every_interface(&self) -> IpAddr {
        let v6 = self.host.parse::<Ipv6Addr>().is_ok();
        if v6 {
            return IpAddr::V6(Ipv6Addr::UNSPECIFIED);
        }
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    }

    pub fn source(&self, http_port: u16) -> String {
        match self.transport {
            Transport::Tcp | Transport::Tls => format!("http://{}", self.authority(http_port)),
            Transport::Ws => format!("http://{}", self.authority(self.port)),
            Transport::Wss => format!("https://{}", self.authority(self.port)),
        }
    }

    pub fn peer_form(&self) -> String {
        match self.transport {
            Transport::Wss => "wss://<its tunnel's name>".to_string(),
            transport => format!("{}://<its name>:{}", transport.name(), self.port),
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}://{}",
            self.transport.name(),
            self.authority(self.port)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_transport_reads_with_its_host_and_port() {
        let cases = [
            (
                "tcp://a.example.org:9000",
                Transport::Tcp,
                "a.example.org",
                9000,
            ),
            (
                "tls://a.example.org:9443",
                Transport::Tls,
                "a.example.org",
                9443,
            ),
            (
                "ws://a.example.org:8844",
                Transport::Ws,
                "a.example.org",
                8844,
            ),
            (
                "wss://a.trycloudflare.com",
                Transport::Wss,
                "a.trycloudflare.com",
                443,
            ),
            (
                "wss://a.trycloudflare.com/",
                Transport::Wss,
                "a.trycloudflare.com",
                443,
            ),
            (
                "tcp://[fd7a:115c::1]:9000",
                Transport::Tcp,
                "fd7a:115c::1",
                9000,
            ),
        ];
        for (text, transport, host, port) in cases {
            let address: Address = text.parse().unwrap();
            let expected = Address {
                transport,
                host: host.into(),
                port,
            };
            assert_eq!(address, expected, "{text}");
            assert_eq!(
                address.to_string().parse::<Address>(),
                Ok(expected),
                "{text}"
            );
        }
    }

    #[test]
    fn a_bare_socket_or_a_foreign_scheme_is_refused() {
        for text in [
            "100.64.1.2:9000",
            "a.example.org:9000",
            "http://a.example.org:8844",
            "tcp://a.example.org",
            "tls://:9443",
            "wss://a.example.org/v1/p2p",
            "wss://user@a.example.org",
            "tcp://a.example.org:http",
            "tcp://a\"b.example.org:9000",
        ] {
            assert!(text.parse::<Address>().is_err(), "{text}");
        }
    }

    #[test]
    fn a_peer_joins_through_the_http_side_of_the_address() {
        let source = |text: &str| text.parse::<Address>().unwrap().source(8844);
        assert_eq!(
            source("tcp://a.example.org:9000"),
            "http://a.example.org:8844"
        );
        assert_eq!(
            source("tls://a.example.org:9443"),
            "http://a.example.org:8844"
        );
        assert_eq!(
            source("ws://a.example.org:8080"),
            "http://a.example.org:8080"
        );
        assert_eq!(
            source("wss://a.trycloudflare.com"),
            "https://a.trycloudflare.com:443"
        );
    }
}
