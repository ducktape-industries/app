//! App-owned workspace files and the invite envelope.
//!
//! These are the small host-side surfaces the app reads and writes.  The
//! consensus node still owns frame/signing protocols; this module only keeps
//! the file formats needed by the desktop shell local to the desktop shell.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use commonware_codec::{DecodeExt as _, Encode as _};
use commonware_cryptography::{Signer as _, Verifier as _, ed25519};
use module_artifact::{
    Artifact, LaneDecl, MAX_ARTIFACT_BYTES, MAX_VIEW_ASSETS, ModuleArtifact, ViewArtifact,
};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const DEFAULT_APP_RPC: &str = "http://127.0.0.1:8844";
pub const DEFAULT_BLOCK_TIME_MS: u64 = 1_000;
pub const DEFAULT_INVITE_TTL_DAYS: u64 = 7;
pub const INVITE_ENVELOPE_NAMESPACE: &[u8] = b"ducktape-invite-envelope";
const INVITE_GRANT_NAMESPACE: &[u8] = b"ducktape-invite-grant-v1";

pub fn validate_module_id(id: &str) -> Result<(), String> {
    let invalid = id.is_empty()
        || id.contains('=')
        || id.contains('\n')
        || id.contains('/')
        || id.contains('\\')
        || id.contains('\0')
        || matches!(id, "." | "..");
    if invalid {
        Err(format!(
            "module id {id:?} is not a bare identifier (empty, a path, or contains '=' / newline)"
        ))
    } else {
        Ok(())
    }
}

fn validate_chain_id_shape(id: &str) -> Result<(), String> {
    let malformed = || format!("chain id {id:?} is not <name>#<8 hex> — refusing to join it");
    let (name, salt) = id.split_once('#').ok_or_else(malformed)?;
    if name.is_empty()
        || salt.len() != 8
        || !salt.bytes().all(|b| b.is_ascii_hexdigit())
        || salt.chars().any(|c| c.is_ascii_uppercase())
    {
        return Err(malformed());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModuleCode {
    pub id: String,
    pub code_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NetworkDescriptor {
    pub chain_id: String,
    pub validators: Vec<String>,
    #[serde(default)]
    pub bootstrap: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reach: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordination: Option<String>,
    pub genesis: String,
    pub block_time_ms: u64,
    pub modules: Vec<ModuleCode>,
}

impl NetworkDescriptor {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let mut descriptor: Self =
            toml::from_str(text).map_err(|e| format!("network descriptor: {e}"))?;
        for validator in &mut descriptor.validators {
            *validator = validator.trim().to_ascii_lowercase();
        }
        descriptor.validators.sort();
        for module in &mut descriptor.modules {
            validate_module_id(&module.id)?;
            module.code_hash = module.code_hash.trim().to_ascii_lowercase();
        }
        descriptor.modules.sort_by(|a, b| a.id.cmp(&b.id));
        descriptor.genesis = descriptor.genesis.trim().to_ascii_lowercase();
        if descriptor.block_time_ms < 100 {
            return Err("block_time_ms must be at least 100ms (the node's idle drain tick)".into());
        }
        Ok(descriptor)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("network descriptor serializes")
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        Self::from_toml(&std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&temporary, self.to_toml().as_bytes())
            .map_err(|e| format!("write {temporary:?}: {e}"))?;
        std::fs::rename(&temporary, path).map_err(|e| format!("publish {path:?}: {e}"))
    }

    fn validator_keys(&self) -> Result<Vec<ed25519::PublicKey>, String> {
        self.validators
            .iter()
            .map(|key| {
                let raw = unhex(key)?;
                ed25519::PublicKey::decode(&raw[..]).map_err(|e| format!("validator key: {e}"))
            })
            .collect()
    }

    fn module_hashes(&self) -> Result<Vec<(String, [u8; 32])>, String> {
        let mut modules = self
            .modules
            .iter()
            .map(|module| {
                let raw = unhex(&module.code_hash)?;
                let hash = raw
                    .try_into()
                    .map_err(|_| format!("module {} code_hash must be 32 bytes", module.id))?;
                Ok((module.id.clone(), hash))
            })
            .collect::<Result<Vec<_>, String>>()?;
        modules.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(modules)
    }

    pub fn genesis_namespace(&self) -> String {
        let mut validators: Vec<String> = self
            .validators
            .iter()
            .map(|key| key.trim().to_ascii_lowercase())
            .collect();
        validators.sort();
        let mut hasher = Sha256::new();
        hasher.update(b"ducktape:genesis:v1:");
        for validator in validators {
            hasher.update(b"\n");
            hasher.update(validator.as_bytes());
        }
        let mut modules: Vec<&ModuleCode> = self.modules.iter().collect();
        modules.sort_by(|a, b| a.id.cmp(&b.id));
        for module in modules {
            hasher.update(b"\n");
            hasher.update(module.id.as_bytes());
            hasher.update(b"=");
            hasher.update(module.code_hash.trim().to_ascii_lowercase().as_bytes());
        }
        hasher.update(b"\ngenesis=");
        hasher.update(self.genesis.trim().to_ascii_lowercase().as_bytes());
        hasher.update(b"\nblock_time_ms=");
        hasher.update(self.block_time_ms.to_string().as_bytes());
        let digest = hasher.finalize();
        format!("{}@{}", self.chain_id, hex_bytes(&digest[..16]))
    }
}

pub fn ducktape_home() -> Result<PathBuf, String> {
    ducktape_home::root()
}

pub fn default_workspace_dir(chain_id: &str) -> Result<PathBuf, String> {
    let name = chain_id.replace(std::path::MAIN_SEPARATOR, "-");
    if name.is_empty() || name == "." || name == ".." || Path::new(&name).components().count() != 1
    {
        return Err(format!(
            "chain id {chain_id:?} is not a workspace directory name"
        ));
    }
    Ok(ducktape_home()?.join(name))
}

pub fn http_base_in(dir: &Path) -> Result<String, String> {
    let (node, _) = load_node_toml(&dir.join("node.toml"))?;
    Ok(http_base_of(&node.http_listen))
}

fn http_base_of(http_listen: &str) -> String {
    let Some((host, port)) = http_listen.rsplit_once(':') else {
        return format!("http://{http_listen}");
    };
    let loopback = if host.starts_with('[') {
        "[::1]"
    } else {
        "127.0.0.1"
    };
    format!("http://{loopback}:{port}")
}

pub fn list_workspaces_in(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {root:?}: {error}")),
    };
    let mut workspaces = Vec::new();
    for entry in entries.flatten() {
        let directory = entry.path();
        let descriptor = directory.join("network.toml");
        if descriptor.is_file()
            && let Ok(network) = NetworkDescriptor::load(&descriptor)
        {
            workspaces.push((network.chain_id, directory.join("node.toml")));
        }
    }
    workspaces.sort();
    Ok(workspaces)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct NodeToml {
    pub network: String,
    pub key_file: String,
    pub listen: String,
    pub advertised: String,
    pub storage_dir: String,
    pub http_listen: String,
    pub gateway_listen: String,
    pub rpc_listen: String,
    pub wireguard_listen: String,
    pub invite_listen: String,
    pub wireguard_advertised: String,
    pub primary_coordinator: String,
    pub coordinator_relay: String,
    pub checkpoint_blocks: u64,
    pub sandbox: Option<toml::Value>,
}

pub fn load_node_toml(path: &Path) -> Result<(NodeToml, PathBuf), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
    let node: NodeToml = toml::from_str(&text).map_err(|e| format!("node.toml: {e}"))?;
    Ok((node, path.parent().unwrap_or(Path::new(".")).to_path_buf()))
}

pub fn load_identity(path: &Path) -> Result<ed25519::PrivateKey, String> {
    let raw = unhex(
        std::fs::read_to_string(path)
            .map_err(|e| format!("read {path:?}: {e}"))?
            .trim(),
    )?;
    ed25519::PrivateKey::decode(&raw[..])
        .map_err(|e| format!("{path:?} is not an ed25519 secret: {e}"))
}

pub fn write_identity(path: &Path, key: &ed25519::PrivateKey) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {parent:?}: {e}"))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    std::io::Write::write_all(
        &mut options
            .open(path)
            .map_err(|e| format!("create {path:?}: {e}"))?,
        format!("{}\n", hex_bytes(key.encode().as_ref())).as_bytes(),
    )
    .map_err(|e| format!("write {path:?}: {e}"))
}

pub fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("string write");
    }
    out
}

fn unhex(input: &str) -> Result<Vec<u8>, String> {
    let input = input.trim();
    if !input.len().is_multiple_of(2) {
        return Err("hex input has odd length".into());
    }
    (0..input.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&input[index..index + 2], 16)
                .map_err(|e| format!("invalid hex: {e}"))
        })
        .collect()
}

pub mod staged_key {
    pub fn staged_set_name(base: &str, checkout: &std::path::Path) -> String {
        let path = checkout.to_string_lossy();
        if !path.starts_with('/') {
            return base.to_owned();
        }
        let mut name = String::with_capacity(base.len() + path.len());
        name.push_str(base);
        for part in path.split('/') {
            if !part.is_empty() {
                name.push('%');
                name.push_str(part);
            }
        }
        name
    }

    pub fn checkout_of_crate(manifest_dir: &std::path::Path) -> std::path::PathBuf {
        let checkout = manifest_dir.join("../..");
        std::fs::canonicalize(&checkout).unwrap_or(checkout)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InviteToken {
    pub issuer: ed25519::PublicKey,
    pub nonce: [u8; 16],
    pub expires_unix_secs: u64,
    pub sig: ed25519::Signature,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InviteWireGuard {
    pub public_key: [u8; 32],
    pub endpoint: Option<String>,
    pub intro: Option<String>,
    pub mesh_port: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Front {
    pub member_key: [u8; 32],
    pub wireguard_public_key: [u8; 32],
    pub mesh_port: u16,
    pub endpoint: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Invite {
    pub descriptor: NetworkDescriptor,
    pub token: InviteToken,
    pub wireguard: InviteWireGuard,
    pub fronts: Vec<Front>,
    pub coordinator: Option<String>,
    pub expires_unix_secs: u64,
}

#[derive(Serialize)]
struct StoredInviteWireGuard {
    issuer: String,
    public_key: String,
    endpoint: Option<String>,
    intro: Option<String>,
    mesh_port: u16,
}

#[derive(Serialize)]
struct StoredFront {
    member_key: String,
    wireguard_public_key: String,
    mesh_port: u16,
    endpoint: Option<String>,
}

pub fn mint_invite_token(
    signer: &ed25519::PrivateKey,
    binding: &[u8],
    expires_unix_secs: u64,
) -> InviteToken {
    let mut nonce = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut message = Vec::with_capacity(binding.len() + 24);
    message.extend_from_slice(binding);
    message.extend_from_slice(&nonce);
    message.extend_from_slice(&expires_unix_secs.to_le_bytes());
    InviteToken {
        issuer: signer.public_key(),
        nonce,
        expires_unix_secs,
        sig: signer.sign(INVITE_GRANT_NAMESPACE, &message),
    }
}

fn pack_token(token: &InviteToken) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(120);
    bytes.extend_from_slice(token.issuer.as_ref());
    bytes.extend_from_slice(&token.nonce);
    bytes.extend_from_slice(&token.expires_unix_secs.to_le_bytes());
    bytes.extend_from_slice(token.sig.encode().as_ref());
    bytes
}

fn unpack_token(bytes: &[u8]) -> Result<InviteToken, String> {
    if bytes.len() != 120 {
        return Err(format!(
            "invite token must be 120 bytes, got {}",
            bytes.len()
        ));
    }
    let issuer = ed25519::PublicKey::decode(&bytes[..32])
        .map_err(|e| format!("invite token issuer: {e}"))?;
    let mut nonce = [0u8; 16];
    nonce.copy_from_slice(&bytes[32..48]);
    let expires_unix_secs = u64::from_le_bytes(bytes[48..56].try_into().expect("8 bytes"));
    let sig = ed25519::Signature::decode(&bytes[56..])
        .map_err(|e| format!("invite token signature: {e}"))?;
    Ok(InviteToken {
        issuer,
        nonce,
        expires_unix_secs,
        sig,
    })
}

pub fn encode_invite(
    descriptor: &NetworkDescriptor,
    token: &InviteToken,
    wireguard: &InviteWireGuard,
    fronts: &[Front],
    coordinator: Option<&str>,
    signer: &ed25519::PrivateKey,
) -> Result<String, String> {
    if signer.public_key() != token.issuer {
        return Err("invite envelope must be signed by the token's issuer".into());
    }
    let mut bytes = Vec::new();
    put_str(&mut bytes, &descriptor.chain_id)?;
    let validators = descriptor.validator_keys()?;
    bytes.push(u8::try_from(validators.len()).map_err(|_| "too many validators".to_string())?);
    for validator in validators {
        bytes.extend_from_slice(validator.as_ref());
    }
    let mut modules = descriptor.module_hashes()?;
    bytes.push(u8::try_from(modules.len()).map_err(|_| "too many modules".to_string())?);
    for (id, hash) in modules.drain(..) {
        put_str(&mut bytes, &id)?;
        bytes.extend_from_slice(&hash);
    }
    let genesis = unhex(&descriptor.genesis)?;
    if genesis.len() != 32 {
        return Err("genesis must be 32 bytes".into());
    }
    bytes.extend_from_slice(&genesis);
    bytes.extend_from_slice(&descriptor.block_time_ms.to_le_bytes());
    bytes.push(0); // The app only creates test descriptors without reach hints.
    match (&wireguard.endpoint, &wireguard.intro) {
        (Some(endpoint), Some(intro)) => {
            bytes.push(1);
            bytes.extend_from_slice(&wireguard.public_key);
            put_str(&mut bytes, endpoint)?;
            put_str(&mut bytes, intro)?;
            bytes.extend_from_slice(&wireguard.mesh_port.to_le_bytes());
        }
        (None, None) => {
            bytes.push(2);
            bytes.extend_from_slice(&wireguard.public_key);
            bytes.extend_from_slice(&wireguard.mesh_port.to_le_bytes());
        }
        _ => return Err("wireguard invite must carry both endpoint and intro, or neither".into()),
    }
    bytes.push(if descriptor.coordination.as_deref() == Some("public") {
        0
    } else {
        1
    });
    match coordinator {
        None => bytes.push(0),
        Some(value) => {
            bytes.push(1);
            put_str(&mut bytes, value)?;
        }
    }
    let token_bytes = pack_token(token);
    bytes.push(token_bytes.len() as u8);
    bytes.extend_from_slice(&token_bytes);
    bytes.push(u8::try_from(fronts.len()).map_err(|_| "too many fronts".to_string())?);
    for front in fronts {
        bytes.extend_from_slice(&front.member_key);
        bytes.extend_from_slice(&front.wireguard_public_key);
        bytes.extend_from_slice(&front.mesh_port.to_le_bytes());
        match &front.endpoint {
            None => bytes.push(0),
            Some(endpoint) => {
                bytes.push(1);
                put_str(&mut bytes, endpoint)?;
            }
        }
    }
    let signature = signer.sign(INVITE_ENVELOPE_NAMESPACE, &bytes);
    bytes.extend_from_slice(signature.encode().as_ref());
    Ok(format!(
        "🦆{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub fn decode_invite(blob: &str) -> Result<Invite, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    decode_invite_at(blob, now)
}

fn decode_invite_at(blob: &str, now: u64) -> Result<Invite, String> {
    let compact: String = blob
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let encoded = compact.strip_prefix("🦆").ok_or_else(|| {
        "not a ducktape invite (expected 🦆...) — ask for a fresh invite".to_string()
    })?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| format!("invite is not valid base64url: {e}"))?;
    let signed_len = bytes
        .len()
        .checked_sub(64)
        .ok_or_else(|| "invite payload truncated".to_string())?;
    let (signed, signature) = bytes.split_at(signed_len);
    let signature =
        ed25519::Signature::decode(signature).map_err(|e| format!("envelope signature: {e}"))?;
    let invite = unpack_invite(signed, now)?;
    if !invite
        .token
        .issuer
        .verify(INVITE_ENVELOPE_NAMESPACE, signed, &signature)
    {
        return Err("invite envelope signature does not verify".into());
    }
    let mut message = Vec::with_capacity(invite.descriptor.genesis_namespace().len() + 24);
    message.extend_from_slice(invite.descriptor.genesis_namespace().as_bytes());
    message.extend_from_slice(&invite.token.nonce);
    message.extend_from_slice(&invite.token.expires_unix_secs.to_le_bytes());
    if !invite
        .token
        .issuer
        .verify(INVITE_GRANT_NAMESPACE, &message, &invite.token.sig)
    {
        return Err("invite token does not verify against this blob's own network — the blob was tampered with".into());
    }
    Ok(invite)
}

fn unpack_invite(bytes: &[u8], now: u64) -> Result<Invite, String> {
    let mut reader = Reader { bytes, position: 0 };
    let chain_id = reader.string()?;
    let validator_count = reader.byte()? as usize;
    let mut validators = Vec::with_capacity(validator_count);
    for _ in 0..validator_count {
        validators.push(hex_bytes(reader.take(32)?));
    }
    validators.sort();
    let module_count = reader.byte()? as usize;
    let mut modules = Vec::with_capacity(module_count);
    for _ in 0..module_count {
        let id = reader.string()?;
        validate_module_id(&id)?;
        modules.push(ModuleCode {
            id,
            code_hash: hex_bytes(reader.take(32)?),
        });
    }
    modules.sort_by(|a, b| a.id.cmp(&b.id));
    let genesis = hex_bytes(reader.take(32)?);
    let block_time_ms = u64::from_le_bytes(reader.take(8)?.try_into().expect("8 bytes"));
    if block_time_ms < 100 {
        return Err("block_time_ms must be at least 100ms (the node's idle drain tick)".into());
    }
    let reach_count = reader.byte()? as usize;
    let mut reach = Vec::with_capacity(reach_count);
    for _ in 0..reach_count {
        let expected = hex_bytes(reader.take(32)?);
        let kind = reader.byte()?;
        let value = match kind {
            0 => format!("direct:{}@{}", expected, reader.string()?),
            1 => {
                let address = reader.string()?;
                let key = hex_bytes(reader.take(32)?);
                format!("coordinated:{}@{}", key, address)
            }
            other => return Err(format!("unknown reach tag {other} in invite")),
        };
        reach.push(value);
    }
    let wireguard =
        match reader.byte()? {
            1 => InviteWireGuard {
                public_key: reader.array32()?,
                endpoint: Some(reader.string()?),
                intro: Some(reader.string()?),
                mesh_port: u16::from_le_bytes(reader.take(2)?.try_into().expect("2 bytes")),
            },
            2 => InviteWireGuard {
                public_key: reader.array32()?,
                endpoint: None,
                intro: None,
                mesh_port: u16::from_le_bytes(reader.take(2)?.try_into().expect("2 bytes")),
            },
            0 => return Err(
                "this invite carries no WireGuard bootstrap — the reachability plane is required"
                    .into(),
            ),
            other => return Err(format!("unknown wireguard flag {other} in invite")),
        };
    let coordination = match reader.byte()? {
        0 => Some("public".to_string()),
        1 => Some("private".to_string()),
        other => return Err(format!("unknown coordination mode {other} in invite")),
    };
    let coordinator = match reader.byte()? {
        0 => None,
        1 => Some(reader.string()?),
        other => return Err(format!("unknown coordinator flag {other} in invite")),
    };
    let token_length = reader.byte()? as usize;
    let token = unpack_token(reader.take(token_length)?)?;
    if now >= token.expires_unix_secs {
        return Err("this invite has expired — ask for a fresh one".into());
    }
    let front_count = reader.byte()? as usize;
    let mut fronts = Vec::with_capacity(front_count);
    for _ in 0..front_count {
        fronts.push(Front {
            member_key: reader.array32()?,
            wireguard_public_key: reader.array32()?,
            mesh_port: u16::from_le_bytes(reader.take(2)?.try_into().expect("2 bytes")),
            endpoint: match reader.byte()? {
                0 => None,
                1 => Some(reader.string()?),
                other => return Err(format!("unknown front endpoint flag {other} in invite")),
            },
        });
    }
    if reader.position != bytes.len() {
        return Err("invite payload has trailing bytes".into());
    }
    Ok(Invite {
        descriptor: NetworkDescriptor {
            chain_id,
            validators,
            bootstrap: Vec::new(),
            reach,
            coordination,
            genesis,
            block_time_ms,
            modules,
        },
        expires_unix_secs: token.expires_unix_secs,
        token,
        wireguard,
        fronts,
        coordinator,
    })
}

fn put_str(out: &mut Vec<u8>, value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    out.push(u8::try_from(bytes.len()).map_err(|_| format!("string too long: {value:?}"))?);
    out.extend_from_slice(bytes);
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let byte = *self
            .bytes
            .get(self.position)
            .ok_or_else(|| "invite payload truncated".to_string())?;
        self.position += 1;
        Ok(byte)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| "invite length overflow".to_string())?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| "invite payload truncated".to_string())?;
        self.position = end;
        Ok(bytes)
    }

    fn string(&mut self) -> Result<String, String> {
        let length = self.byte()? as usize;
        String::from_utf8(self.take(length)?.to_vec()).map_err(|e| format!("invite string: {e}"))
    }

    fn array32(&mut self) -> Result<[u8; 32], String> {
        self.take(32)?
            .try_into()
            .map_err(|_| "invite key must be 32 bytes".to_string())
    }
}

#[derive(Clone, Debug)]
pub struct JoinedWorkspace {
    pub chain_id: String,
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct PlumbingOverrides;

pub fn join_workspace(
    blob: &str,
    dir: Option<PathBuf>,
    _overrides: &PlumbingOverrides,
) -> Result<JoinedWorkspace, String> {
    let invite = decode_invite(blob)?;
    validate_chain_id_shape(&invite.descriptor.chain_id)?;
    let dir = match dir {
        Some(dir) => dir,
        None => default_workspace_dir(&invite.descriptor.chain_id)?,
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let identity_path = dir.join("identity.key");
    if !identity_path.exists() {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let identity =
            ed25519::PrivateKey::decode(&bytes[..]).map_err(|e| format!("identity: {e}"))?;
        write_identity(&identity_path, &identity)?;
    }
    invite.descriptor.save(&dir.join("network.toml"))?;
    let node_toml = format!(
        "network = {network:?}\nkey_file = \"identity.key\"\nlisten = \"0.0.0.0:52200\"\nadvertised = \"overlay\"\nstorage_dir = \"data\"\nhttp_listen = \"0.0.0.0:8844\"\ngateway_listen = \"127.0.0.1:0\"\nrpc_listen = \"127.0.0.1:8845\"\nwireguard_listen = \"0.0.0.0:51820\"\ninvite_listen = \"0.0.0.0:51821\"\nwireguard_advertised = \"auto\"\nprimary_coordinator = \"none\"\ncoordinator_relay = \"none\"\ncheckpoint_blocks = 32\n",
        network = "network.toml"
    );
    let node_path = dir.join("node.toml");
    if node_path.exists() {
        load_node_toml(&node_path)?;
    } else {
        std::fs::write(&node_path, node_toml).map_err(|e| format!("write node.toml: {e}"))?;
    }
    write_private_text(
        &dir.join("invite.token"),
        &format!("{}\n", hex_bytes(&pack_token(&invite.token))),
    )?;
    save_invite_wireguard(&dir, &invite.token.issuer, &invite.wireguard)?;
    save_invite_fronts(&dir, &invite.fronts)?;
    write_wireguard_key(&dir.join("wireguard.key"))?;
    Ok(JoinedWorkspace {
        chain_id: invite.descriptor.chain_id,
        dir,
    })
}

fn save_invite_wireguard(
    dir: &Path,
    issuer: &ed25519::PublicKey,
    wireguard: &InviteWireGuard,
) -> Result<(), String> {
    let stored = StoredInviteWireGuard {
        issuer: hex_bytes(issuer.as_ref()),
        public_key: hex_bytes(&wireguard.public_key),
        endpoint: wireguard.endpoint.clone(),
        intro: wireguard.intro.clone(),
        mesh_port: wireguard.mesh_port,
    };
    let path = dir.join("invite-wireguard.toml");
    let text = toml::to_string_pretty(&stored).map_err(|e| format!("encode {path:?}: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("write {path:?}: {e}"))
}

fn save_invite_fronts(dir: &Path, fronts: &[Front]) -> Result<(), String> {
    let path = dir.join("invite-fronts.json");
    if fronts.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("remove {path:?}: {error}")),
        }
        return Ok(());
    }
    let stored = fronts
        .iter()
        .map(|front| StoredFront {
            member_key: hex_bytes(&front.member_key),
            wireguard_public_key: hex_bytes(&front.wireguard_public_key),
            mesh_port: front.mesh_port,
            endpoint: front.endpoint.clone(),
        })
        .collect::<Vec<_>>();
    let text =
        serde_json::to_string_pretty(&stored).map_err(|e| format!("encode {path:?}: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("write {path:?}: {e}"))
}

fn write_wireguard_key(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let mut secret = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(format!("create {path:?}: {error}")),
    };
    std::io::Write::write_all(&mut file, format!("{}\n", hex_bytes(&secret)).as_bytes())
        .map_err(|e| format!("write {path:?}: {e}"))
}

fn write_private_text(path: &Path, text: &str) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| format!("create {path:?}: {e}"))?;
    std::io::Write::write_all(&mut file, text.as_bytes())
        .map_err(|e| format!("write {path:?}: {e}"))
}

pub fn read_module_artifact(dir: &Path, id: &str) -> Result<Artifact, String> {
    validate_module_id(id)?;
    let component = optional_file(dir.join(format!("{id}.component.wasm")))?;
    let index = optional_file(dir.join(format!("{id}.index.wasm")))?;
    let view = optional_file(dir.join(format!("{id}.view.wasm")))?;
    let assets = optional_file(dir.join(format!("{id}.assets")))?;
    let lanes = optional_file(dir.join(format!("{id}.lanes")))?;
    if component.is_none() && view.is_none() {
        return Err(format!(
            "{}: no such founding entry",
            dir.join(format!("{id}.component.wasm")).display()
        ));
    }
    let mut remaining = MAX_ARTIFACT_BYTES;
    let component = component
        .map(|path| read_file(&path, &mut remaining))
        .transpose()?;
    let index = index
        .map(|path| read_file(&path, &mut remaining))
        .transpose()?;
    let view = view
        .map(|path| {
            Ok::<ViewArtifact, String>(ViewArtifact {
                component: read_file(&path, &mut remaining)?,
                assets: assets
                    .map(|path| read_assets(&path, &mut remaining))
                    .transpose()?
                    .unwrap_or_default(),
            })
        })
        .transpose()?;
    let lanes: Vec<LaneDecl> = lanes
        .map(|path| {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())
        })
        .transpose()?
        .unwrap_or_default();
    let artifact = match (component, view) {
        (Some(component), view) => Artifact::Module(ModuleArtifact {
            component,
            index,
            view,
            lanes,
        }),
        (None, Some(view)) => Artifact::View(view),
        (None, None) => return Err("a deployment needs a component or a view".into()),
    };
    Artifact::decode(&artifact.encode())
}

fn optional_file(path: PathBuf) -> Result<Option<PathBuf>, String> {
    match std::fs::symlink_metadata(&path) {
        Ok(_) => Ok(Some(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("inspect {}: {error}", path.display())),
    }
}

fn read_file(path: &Path, remaining: &mut usize) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if bytes.len() > *remaining {
        return Err("deployment files exceed the 16 MiB limit".into());
    }
    *remaining -= bytes.len();
    if bytes.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    Ok(bytes)
}

fn read_assets(root: &Path, remaining: &mut usize) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    fn visit(
        root: &Path,
        path: &Path,
        files: &mut BTreeMap<String, Vec<u8>>,
        remaining: &mut usize,
    ) -> Result<(), String> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|e| format!("inspect {}: {e}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("asset symlink: {}", path.display()));
        }
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                visit(
                    root,
                    &entry.map_err(|e| e.to_string())?.path(),
                    files,
                    remaining,
                )?;
            }
            return Ok(());
        }
        if files.len() >= MAX_VIEW_ASSETS {
            return Err("more than 4096 view assets".into());
        }
        let name = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .iter()
            .map(|part| part.to_str().ok_or("non-UTF-8 asset path"))
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        module_artifact::validate_asset_path(&name)?;
        files.insert(name, read_file(path, remaining)?);
        Ok(())
    }
    visit(root, root, &mut files, remaining)?;
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_id_validator_pins_core_error_text() {
        assert_eq!(
            validate_module_id("chat/view").unwrap_err(),
            "module id \"chat/view\" is not a bare identifier (empty, a path, or contains '=' / newline)"
        );
        assert!(validate_module_id("chat_view").is_ok());
    }

    #[test]
    fn descriptor_fingerprint_pins_core_golden_string() {
        let descriptor = NetworkDescriptor {
            chain_id: "ducktape#a1b2c3d4".into(),
            validators: vec!["bb".repeat(32), "aa".repeat(32)],
            bootstrap: Vec::new(),
            reach: Vec::new(),
            coordination: None,
            genesis: "ab".repeat(32),
            block_time_ms: DEFAULT_BLOCK_TIME_MS,
            modules: Vec::new(),
        };
        assert_eq!(
            descriptor.genesis_namespace(),
            "ducktape#a1b2c3d4@2cc5f11e3e9d2c53fee2f4ec787c5890"
        );
    }

    #[test]
    fn invite_decoder_pins_core_truncated_envelope_error() {
        assert_eq!(decode_invite("🦆").unwrap_err(), "invite payload truncated");
    }
}
