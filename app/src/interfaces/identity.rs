use serde::{Deserialize, Serialize};

pub type AccountNumber = u64;
pub const IDENTITY_ADD_KEY_NS: &[u8] = b"ducktape-identity-add-key-v1";
pub const MAX_NAME_LEN: usize = 64;
pub const MAX_LABEL_LEN: usize = 64;
pub const MAX_QUERY_LIMIT: u64 = 256;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum KeyScheme {
    Ed25519,
    Secp256k1,
    Secp256r1,
}

impl KeyScheme {
    pub const fn tag(self) -> u8 {
        match self {
            Self::Ed25519 => 0,
            Self::Secp256k1 => 1,
            Self::Secp256r1 => 2,
        }
    }
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Ed25519),
            1 => Some(Self::Secp256k1),
            2 => Some(Self::Secp256r1),
            _ => None,
        }
    }
    pub fn pubkey_wellformed(self, pubkey: &[u8]) -> bool {
        match self {
            Self::Ed25519 => pubkey.len() == 32,
            Self::Secp256k1 => {
                pubkey.len() == 33 && k256::ecdsa::VerifyingKey::from_sec1_bytes(pubkey).is_ok()
            }
            Self::Secp256r1 => {
                pubkey.len() == 33 && p256::ecdsa::VerifyingKey::from_sec1_bytes(pubkey).is_ok()
            }
        }
    }
    pub fn verify(self, pubkey: &[u8], namespace: &[u8], preimage: &[u8], proof: &[u8]) -> bool {
        match self {
            Self::Ed25519 => {
                if !self.pubkey_wellformed(pubkey) || proof.len() != 64 {
                    return false;
                }
                use commonware_codec::DecodeExt as _;
                use commonware_cryptography::{
                    Verifier as _,
                    ed25519::{PublicKey, Signature},
                };
                let (Ok(key), Ok(signature)) =
                    (PublicKey::decode(pubkey), Signature::decode(proof))
                else {
                    return false;
                };
                key.verify(namespace, preimage, &signature)
            }
            Self::Secp256r1 => verify_webauthn(pubkey, namespace, preimage, proof),
            Self::Secp256k1 => false,
        }
    }
}

fn verify_webauthn(pubkey: &[u8], namespace: &[u8], preimage: &[u8], proof: &[u8]) -> bool {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier as _};
    use sha2::{Digest as _, Sha256};
    #[derive(Deserialize)]
    struct ClientData {
        #[serde(rename = "type")]
        kind: String,
        challenge: String,
    }
    fn take<'a>(bytes: &mut &'a [u8]) -> Option<&'a [u8]> {
        let (length, rest) = bytes.split_at_checked(4)?;
        let length = u32::from_le_bytes(length.try_into().ok()?) as usize;
        let (value, rest) = rest.split_at_checked(length)?;
        *bytes = rest;
        Some(value)
    }
    let mut rest = proof;
    let Some(authenticator_data) = take(&mut rest) else {
        return false;
    };
    let Some(client_data) = take(&mut rest) else {
        return false;
    };
    if rest.len() != 64 || authenticator_data.len() < 37 || authenticator_data[32] & 1 == 0 {
        return false;
    }
    let Ok(client) = serde_json::from_slice::<ClientData>(client_data) else {
        return false;
    };
    let Ok(challenge) = URL_SAFE_NO_PAD.decode(client.challenge.as_bytes()) else {
        return false;
    };
    let mut challenge_input = Vec::with_capacity(namespace.len() + preimage.len());
    challenge_input.extend_from_slice(namespace);
    challenge_input.extend_from_slice(preimage);
    if challenge != Sha256::digest(challenge_input).as_slice() || client.kind != "webauthn.get" {
        return false;
    }
    let Ok(key) = VerifyingKey::from_sec1_bytes(pubkey) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(rest) else {
        return false;
    };
    let mut signed = Vec::with_capacity(authenticator_data.len() + 32);
    signed.extend_from_slice(authenticator_data);
    signed.extend_from_slice(&Sha256::digest(client_data));
    key.verify(&signed, &signature).is_ok()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeyView {
    pub scheme: KeyScheme,
    pub pubkey: Vec<u8>,
    pub label: Option<String>,
    pub added_at: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProgramStanding {
    Active,
    Suspended,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Keys,
    Program {
        controller: AccountNumber,
        executor: String,
        generation: u64,
        standing: ProgramStanding,
    },
    Revoked {
        controller: AccountNumber,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccountView {
    pub number: AccountNumber,
    pub name: String,
    pub control: Control,
    pub keys: Vec<KeyView>,
    pub avatar: Option<String>,
    pub bio: Option<String>,
    pub updated_at: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Authorizer {
    pub key: Vec<u8>,
    pub account: AccountNumber,
    pub expires_at: u64,
    pub proof: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityMsg {
    Create {
        name: String,
        scheme: KeyScheme,
    },
    AddKey {
        scheme: KeyScheme,
        label: Option<String>,
        authorizer: Authorizer,
    },
    RemoveKey {
        key: Vec<u8>,
    },
    SetName {
        name: String,
    },
    SetProfile {
        avatar: Option<String>,
        bio: Option<String>,
    },
    CreateProgram {
        name: String,
        controller: AccountNumber,
        request: u64,
    },
    SetProgramStanding {
        account: AccountNumber,
        standing: ProgramStanding,
    },
    TransferControl {
        account: AccountNumber,
        to: AccountNumber,
    },
    RevokeProgram {
        account: AccountNumber,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AccountRef {
    Account(AccountNumber),
    Key(Vec<u8>),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityQuery {
    All {
        from: u64,
        limit: u64,
    },
    Get {
        number: AccountNumber,
    },
    OfKey {
        key: Vec<u8>,
    },
    Resolve {
        references: Vec<AccountRef>,
    },
    KeyGen {
        key: Vec<u8>,
    },
    Controlled {
        by: AccountNumber,
        from: u64,
        limit: u64,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityReply {
    Accounts(Vec<AccountView>),
    Account(Option<AccountView>),
    Resolved(Vec<Option<AccountNumber>>),
    Gen(u64),
}

pub fn encode_msg(message: &IdentityMsg) -> Vec<u8> {
    sdk::wire::encode(message)
}
pub fn decode_msg(bytes: &[u8]) -> Result<IdentityMsg, String> {
    sdk::wire::decode(bytes)
}
pub fn encode_query(query: &IdentityQuery) -> Vec<u8> {
    sdk::wire::encode(query)
}
pub fn decode_reply(bytes: &[u8]) -> Result<IdentityReply, String> {
    sdk::wire::decode(bytes)
}

pub fn add_key_preimage(
    chain_id: &str,
    scheme: KeyScheme,
    new_key: &[u8],
    generation: u64,
    account: AccountNumber,
    expires_at: u64,
) -> Vec<u8> {
    let mut out = Vec::new();
    sdk::codec::push_bytes(&mut out, chain_id.as_bytes());
    out.push(scheme.tag());
    sdk::codec::push_bytes(&mut out, new_key);
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&account.to_le_bytes());
    out.extend_from_slice(&expires_at.to_le_bytes());
    out
}
