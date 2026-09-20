use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GATEWAY_CALLER_NS: &[u8] = b"ducktape-gateway-caller-v1";
pub const MAX_RESPONSE_BODY_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_PROXY_HEAD_BYTES: usize = 8192;
pub const MAX_PATH_AND_QUERY_BYTES: usize = 2048;
pub const MAX_HEADERS: usize = 32;
pub const MAX_HEADER_NAME_BYTES: usize = 64;
pub const MAX_HEADER_VALUE_BYTES: usize = 4096;
pub const MAX_HEADER_BYTES: usize = 16384;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(deny_unknown_fields)]
pub struct RouteName {
    pub label: Option<String>,
}
impl RouteName {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(label) = &self.label
            && (label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
        {
            return Err(format!("invalid route label: {label:?}"));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
}
impl RouteMethod {
    pub const fn as_http_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteAudience {
    Owner,
    Network,
    Accounts { account_ids: Vec<u64> },
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RoutePolicy {
    pub audience: RouteAudience,
    pub methods: Vec<RouteMethod>,
    pub max_request_bytes: Option<u64>,
    pub max_response_bytes: u64,
    pub allow_authorization: bool,
    pub allow_upgrade: bool,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteTarget {
    DuckFs { manifest_sha256: String },
    LoopbackHttp,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RouteDefinition {
    pub target: RouteTarget,
    pub policy: RoutePolicy,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RouteStatement {
    pub chain_id: String,
    pub account_id: u64,
    pub name: RouteName,
    pub publisher_node: Vec<u8>,
    pub revision: u64,
    pub route: Option<RouteDefinition>,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemberAuthorization {
    pub signer: Vec<u8>,
    pub signature: Vec<u8>,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RouteRecord {
    pub statement: RouteStatement,
    pub authorization: MemberAuthorization,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProxyHeader {
    pub name: String,
    pub value: String,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UserPop {
    pub key: Vec<u8>,
    pub ts: u64,
    pub sig: Vec<u8>,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProxyRequestHead {
    #[serde(default)]
    pub operator: bool,
    pub account_id: u64,
    pub name: RouteName,
    pub revision: u64,
    pub method: RouteMethod,
    pub path_and_query: String,
    pub headers: Vec<ProxyHeader>,
    pub upgrade: bool,
    pub user_pop: Option<UserPop>,
}

pub fn validate_proxy_request_head(head: &ProxyRequestHead) -> Result<(), String> {
    if head.account_id == 0 {
        return Err("account number must be non-zero".into());
    }
    head.name.validate()?;
    if head.revision == 0 {
        return Err("proxy: revision starts at 1".into());
    }
    validate_origin_form(&head.path_and_query)?;
    validate_headers(&head.headers, "request")?;
    if head.upgrade && !matches!(head.method, RouteMethod::Get) {
        return Err("proxy: a WebSocket upgrade must be a bodyless GET".into());
    }
    Ok(())
}
pub fn encode_proxy_request_head(head: &ProxyRequestHead) -> Result<Vec<u8>, String> {
    validate_proxy_request_head(head)?;
    let bytes = serde_json::to_vec(head).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_PROXY_HEAD_BYTES {
        return Err(format!("proxy: head exceeds {MAX_PROXY_HEAD_BYTES} bytes"));
    }
    Ok(bytes)
}
pub fn validate_origin_form(value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value.starts_with('/')
        || value.starts_with("//")
        || value.len() > MAX_PATH_AND_QUERY_BYTES
        || value.contains(['\r', '\n', '\\', '#'])
        || !value.bytes().all(|byte| (b'!'..=b'~').contains(&byte))
    {
        return Err("proxy: invalid origin-form path/query".into());
    }
    let path = value.split('?').next().unwrap_or(value);
    if path.split('/').any(|segment| {
        matches!(
            segment.to_ascii_lowercase().as_str(),
            "." | ".." | "%2e" | ".%2e" | "%2e." | "%2e%2e"
        )
    }) {
        return Err("proxy: path contains a URL-normalized segment".into());
    }
    Ok(())
}
pub fn validate_headers(headers: &[ProxyHeader], kind: &str) -> Result<(), String> {
    if headers.len() > MAX_HEADERS {
        return Err(format!(
            "proxy: too many {kind} headers (max {MAX_HEADERS})"
        ));
    }
    let mut previous = None;
    let mut total = 0usize;
    for header in headers {
        let bad = header
            .name
            .bytes()
            .position(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'));
        if header.name.is_empty() || header.name.len() > MAX_HEADER_NAME_BYTES || bad.is_some() {
            return Err(format!(
                "proxy: malformed {kind} header name (len {}, first invalid byte at {})",
                header.name.len(),
                bad.map_or_else(|| "none".into(), |at| at.to_string())
            ));
        }
        if header.name.starts_with("x-duck-") {
            return Err(format!(
                "proxy: {kind} header {:?} spoofs a proxy-minted header",
                header.name
            ));
        }
        if previous.is_some_and(|old: &str| old >= header.name.as_str()) {
            return Err(format!(
                "proxy: {kind} headers must be sorted, and unique except set-cookie"
            ));
        }
        if header.value.is_empty()
            || header.value.len() > MAX_HEADER_VALUE_BYTES
            || !header.value.is_ascii()
            || header.value.bytes().any(|byte| byte < b' ' || byte == 0x7f)
        {
            return Err(format!(
                "proxy: invalid value for {kind} header {:?}",
                header.name
            ));
        }
        previous = Some(header.name.as_str());
        total = total
            .checked_add(header.name.len() + header.value.len())
            .ok_or_else(|| "proxy: header size overflow".to_string())?;
    }
    if total > MAX_HEADER_BYTES {
        return Err(format!(
            "proxy: {kind} headers exceed {MAX_HEADER_BYTES} bytes"
        ));
    }
    Ok(())
}

pub fn body_digest(body: &[u8]) -> [u8; 32] {
    Sha256::digest(body).into()
}
pub fn caller_pop_preimage(
    publisher_node: &[u8],
    head: &ProxyRequestHead,
    digest: &[u8; 32],
    ts: u64,
) -> Vec<u8> {
    let mut out = Vec::new();
    sdk::codec::push_bytes(&mut out, publisher_node);
    out.extend_from_slice(&head.account_id.to_le_bytes());
    sdk::codec::push_opt_str(&mut out, head.name.label.as_deref());
    out.extend_from_slice(&head.revision.to_le_bytes());
    sdk::codec::push_bytes(&mut out, head.method.as_http_str().as_bytes());
    sdk::codec::push_bytes(&mut out, head.path_and_query.as_bytes());
    out.extend_from_slice(&(head.headers.len() as u64).to_le_bytes());
    for header in &head.headers {
        sdk::codec::push_bytes(&mut out, header.name.as_bytes());
        sdk::codec::push_bytes(&mut out, header.value.as_bytes());
    }
    out.push(u8::from(head.upgrade));
    out.extend_from_slice(digest);
    out.extend_from_slice(&ts.to_le_bytes());
    out
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GatewayQuery {
    Get { account_id: u64, name: RouteName },
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GatewayReply {
    Resolved(serde_json::Value),
    Registrations(serde_json::Value),
    Route(Box<Option<RouteRecord>>),
    Routes(serde_json::Value),
    Credential(serde_json::Value),
    Credentials(serde_json::Value),
}
