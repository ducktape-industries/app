//! Invites to the network: a grant signed by the key of the validator node this
//! device runs, read from the node's workspace. The Account view's Invites pane mints them.

use std::path::Path;

use abi::role::validators::{Grant, INVITE_NAMESPACE, Invite};
use commonware_codec::DecodeExt as _;
use commonware_cryptography::{Signer as _, ed25519};
use view_wire::methods::refusal;

use noded_client::hex::{hex_decode, hex_encode};
use noded_client::wire::Layer;

use super::{RpcClient, query_frame, refused};

const NODE_KEY: &str = "identity.key";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

pub(crate) fn node_key(workspace: &Path) -> Result<Option<ed25519::PrivateKey>, String> {
    let path = workspace.join(NODE_KEY);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let seed = hex_decode(text.trim()).map_err(|error| format!("{}: {error}", path.display()))?;
    let key = ed25519::PrivateKey::decode(seed.as_slice())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Some(key))
}

pub(crate) async fn mint(
    client: &RpcClient,
    key: Option<ed25519::PrivateKey>,
    ttl_days: u64,
    now_ms: u64,
) -> Result<String, view_wire::Error> {
    let status = client.status().await.map_err(refused)?;
    let runs_this_node =
        |key: &ed25519::PrivateKey| key.public_key().as_ref() == status.identity.as_slice();
    let Some(key) = key.filter(runs_this_node) else {
        return Err(view_wire::Error::new(
            refusal::INVITE_UNSUPPORTED,
            "Only the machine running this node can mint its invites.",
        ));
    };
    let validators = validators(client, &status.network, &status.roles.validators).await?;
    let validates = validators.contains(&status.identity);
    if !validates {
        return Err(view_wire::Error::new(
            refusal::INVITE_UNSUPPORTED,
            "Only a validator's node can mint invites, and this node is a resident.",
        ));
    }
    let grant = Grant {
        network: status.network.into_bytes(),
        nonce: rand::random(),
        expires: now_ms + ttl_days * DAY_MS,
    };
    let signature = key.sign(INVITE_NAMESPACE, &grant.preimage());
    let invite = Invite {
        issuer: status.identity,
        grant,
        signature: signature.as_ref().to_vec(),
    };
    Ok(hex_encode(&abi::encode(&invite)))
}

async fn validators(
    client: &RpcClient,
    network: &str,
    program: &str,
) -> Result<Vec<Vec<u8>>, view_wire::Error> {
    let frame = query_frame(network, program, abi::encode(&valset::Query::Validators)).await;
    let reply = client
        .query(Layer::Preconfirmed, frame)
        .await
        .map_err(refused)?;
    match abi::decode(&reply) {
        Ok(valset::Reply::Validators(validators)) => Ok(validators),
        Ok(_) => Err(view_wire::Error::new(
            view_wire::code::UNEXPECTED_REPLY,
            "the validators program answered another question",
        )),
        Err(refusal) => Err(view_wire::Error::new(
            view_wire::code::UNEXPECTED_REPLY,
            refusal.sentence,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_node_key_is_the_workspaces_hex_seed_and_absent_without_one() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(node_key(workspace.path()).unwrap().is_none());
        let seed = [9u8; 32];
        std::fs::write(
            workspace.path().join(NODE_KEY),
            format!("{}\n", hex_encode(&seed)),
        )
        .unwrap();
        let key = node_key(workspace.path()).unwrap().unwrap();
        let expected = ed25519::PrivateKey::decode(seed.as_slice()).unwrap();
        assert_eq!(key.public_key(), expected.public_key());
        std::fs::write(workspace.path().join(NODE_KEY), "not hex").unwrap();
        assert!(node_key(workspace.path()).is_err());
    }
}
