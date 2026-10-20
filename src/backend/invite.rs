//! Invites to the network: a grant signed by the key of the validator node this
//! device runs, read from the node's workspace. The Account view's Invites pane mints them.

use std::path::Path;

use abi::role::validators::{Grant, INVITE_NAMESPACE, Invite};
use commonware_codec::DecodeExt as _;
use commonware_cryptography::{Signer as _, ed25519};
use view_wire::methods::refusal;

use noded_client::hex::{hex_decode, hex_encode};
use noded_client::wire::Layer;

use super::{RpcClient, query_frame, query_signer, refused};

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
    let role = status
        .roles
        .bound(abi::role::VALIDATORS)
        .map_err(|refusal| view_wire::Error::new(refusal.reason, refusal.sentence))?;
    let validators = validators(client, &status.network, role).await?;
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
    let validators = abi::encode(&valset::Query::Validators);
    let frame = query_frame(&*query_signer(), network, program, validators).await?;
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

    fn local_node_serving_routes(routes: Vec<(&'static str, Vec<u8>)>) -> RpcClient {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = RpcClient::new(format!("http://{}", listener.local_addr().unwrap()));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut request = String::new();
                    std::io::BufReader::new(&stream)
                        .read_line(&mut request)
                        .unwrap();
                    let path = request.split_whitespace().nth(1).unwrap_or_default();
                    let body = routes
                        .iter()
                        .find(|(route, _)| path.starts_with(route))
                        .map(|(_, body)| body.clone())
                        .unwrap_or_default();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(&body).unwrap();
                });
            }
        });
        client
    }

    fn seeded_key(seed: u8) -> commonware_cryptography::ed25519::PrivateKey {
        use commonware_codec::DecodeExt as _;
        commonware_cryptography::ed25519::PrivateKey::decode([seed; 32].as_slice()).unwrap()
    }

    fn validator_node(identity: &[u8], validators: Vec<Vec<u8>>) -> RpcClient {
        let status = noded_client::wire::Status {
            network: "test-network".into(),
            time: 12,
            block_time_ms: 1000,
            epoch_length: 64,
            height: 9,
            tip: [2; 32],
            root: abi::Root([3; 32]),
            epoch: 0,
            identity: identity.to_vec(),
            contract: noded_client::wire::NODE_CONTRACT,
            genesis: [5; 32],
            roles: crate::backend::test_roles(),
        };
        local_node_serving_routes(vec![
            (noded_client::wire::route::STATUS, abi::encode(&status)),
            (
                noded_client::wire::route::QUERY,
                abi::encode(&abi::encode(&valset::Reply::Validators(validators))),
            ),
        ])
    }

    #[tokio::test]
    async fn an_invite_is_the_validator_nodes_signed_grant_for_its_network() {
        use abi::role::validators::{INVITE_NAMESPACE, Invite};
        use commonware_codec::DecodeExt as _;
        use commonware_cryptography::{Signer as _, Verifier as _, ed25519};
        let key = seeded_key(7);
        let identity = key.public_key().as_ref().to_vec();
        let held = validator_node(&identity, vec![vec![1; 32], identity.clone()]);
        let now = 1_000_000;
        let hex = mint(&held, Some(key.clone()), 7, now).await.unwrap();
        let invite: Invite = abi::decode(&noded_client::hex::hex_decode(&hex).unwrap()).unwrap();
        assert_eq!(invite.issuer, identity);
        assert_eq!(invite.grant.network, b"test-network");
        assert_eq!(invite.grant.expires, now + 7 * 24 * 60 * 60 * 1000);
        let signature = ed25519::Signature::decode(invite.signature.as_slice()).unwrap();
        assert!(
            key.public_key()
                .verify(INVITE_NAMESPACE, &invite.grant.preimage(), &signature)
        );
        let again = mint(&held, Some(key), 7, now).await.unwrap();
        let again: Invite = abi::decode(&noded_client::hex::hex_decode(&again).unwrap()).unwrap();
        assert_ne!(again.grant.nonce, invite.grant.nonce);
    }

    #[tokio::test]
    async fn only_the_machine_running_a_validator_node_mints_its_invites() {
        use commonware_cryptography::Signer as _;
        let key = seeded_key(7);
        let identity = key.public_key().as_ref().to_vec();
        let held = validator_node(&identity, vec![identity.clone()]);
        for key in [None, Some(seeded_key(8))] {
            let refusal = mint(&held, key, 7, 0).await.unwrap_err();
            assert_eq!(refusal.code, refusal::INVITE_UNSUPPORTED);
            assert_eq!(
                refusal.message,
                "Only the machine running this node can mint its invites."
            );
        }
        let resident = validator_node(&identity, vec![vec![1; 32]]);
        let refusal = mint(&resident, Some(key), 7, 0).await.unwrap_err();
        assert_eq!(refusal.code, refusal::INVITE_UNSUPPORTED);
        assert_eq!(
            refusal.message,
            "Only a validator's node can mint invites, and this node is a resident."
        );
    }
}
