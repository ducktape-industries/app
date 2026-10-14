//! The `/v1` surface `noded` serves, borsh in and out, and the signed frame
//! every write and query carries.
//!
//! The types here mirror `crates/noded/src/wire.rs` and
//! `crates/kernel/node/src/frame.rs` on the kernel branch field for field.
//! They are copied, not linked, because the daemon's crates carry the whole
//! kernel (host, state, consensus) and a client has no use for it.
//! ponytail: a `noded-wire` leaf crate in ducktape would end the copy; ask
//! for one when a field moves.
//!
//! Nothing here knows a program. A target is whatever string the caller
//! hands over.

use abi::{ProgramId, Root};
use borsh::{BorshDeserialize, BorshSerialize};

use crate::Signer;

pub const NODE_CONTRACT: u32 = 4;
pub const FRAME_NAMESPACE: &[u8] = b"ducktape:frame";

pub mod route {
    pub const STATUS: &str = "/v1/status";
    pub const SUBMIT: &str = "/v1/submit";
    pub const QUERY: &str = "/v1/query";
    pub const BLOB_GET: &str = "/v1/blob/get";
    pub const CHANGES: &str = "/v1/changes";
    pub const BLOCKS: &str = "/v1/blocks";
    pub const BLOCK: &str = "/v1/block";
    pub const TX: &str = "/v1/tx";
    pub const NETWORK: &str = "/v1/network";
}

/// `host::Layer`: which state a read sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Layer {
    Confirmed,
    Preconfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Tip {
    pub height: u64,
    pub id: [u8; 32],
}

/// The anchor a query's frame carries: a query reads, so it is anchored to
/// no block.
pub const UNANCHORED: Tip = Tip {
    height: 0,
    id: [0; 32],
};

/// A fresh salt for a write's frame: two writes of the same bytes are two
/// transactions.
pub fn salt() -> u64 {
    rand::random()
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Body {
    /// The signer's scheme: a device key is `Ed25519`, a passkey
    /// `Secp256r1`. The node decodes it as core's `KeyScheme`, whose three
    /// tags are `abi::Scheme`'s first three (0, 1, 2), so a body never
    /// carries `Bls12381`.
    pub scheme: abi::Scheme,
    pub signer: Vec<u8>,
    pub network: Vec<u8>,
    pub anchor: Tip,
    pub salt: Option<u64>,
    pub target: String,
    pub payload: Vec<u8>,
}

impl Body {
    /// What the signer signs under [`FRAME_NAMESPACE`].
    pub fn preimage(&self) -> Vec<u8> {
        abi::encode(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Frame {
    pub body: Body,
    pub proof: Vec<u8>,
}

impl Frame {
    pub fn sign(
        signer: &dyn Signer,
        network: &[u8],
        anchor: Tip,
        salt: Option<u64>,
        target: &str,
        payload: Vec<u8>,
    ) -> Frame {
        let body = Body {
            scheme: abi::Scheme::Ed25519,
            signer: signer.public_key().to_vec(),
            network: network.to_vec(),
            anchor,
            salt,
            target: target.to_owned(),
            payload,
        };
        let proof = signer.sign(FRAME_NAMESPACE, &body.preimage()).to_vec();
        Frame { body, proof }
    }

    /// A query's frame: [`UNANCHORED`], no salt.
    pub fn query(signer: &dyn Signer, network: &[u8], target: &str, payload: Vec<u8>) -> Frame {
        Frame::sign(signer, network, UNANCHORED, None, target, payload)
    }

    pub fn encode(&self) -> Vec<u8> {
        abi::encode(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Status {
    pub network: String,
    /// The chain's founding time (its genesis block's), unix ms — not now.
    /// With `network`, what [`keyring::bind`](crate::keyring::bind) tells
    /// two chains apart by.
    pub time: u64,
    pub block_time_ms: u64,
    pub epoch_length: u64,
    pub height: u64,
    pub tip: [u8; 32],
    pub root: Root,
    pub epoch: u64,
    pub identity: Vec<u8>,
    pub contract: u32,
    /// The genesis block's digest: with the network name, the chain's id.
    pub genesis: [u8; 32],
    pub roles: abi::Roles,
}

impl Status {
    pub fn anchor(&self) -> Tip {
        Tip {
            height: self.height,
            id: self.tip,
        }
    }
}

/// Every member of the current epoch as the node sees it (`/v1/network`):
/// its applied tip when it answered, and the members in key order. No
/// member's `signed` exceeds `height`.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Network {
    pub height: u64,
    pub members: Vec<PeerStatus>,
}

/// One member: the newest block the node applied that this key sent a
/// finalize vote for, as the node's consensus engine heard it. A vote for a
/// block not applied yet counts once it is. `None` for a resident, for a
/// validator not heard since the node started or began validating, and for
/// every member while the node is not itself seated as a validator: it runs
/// no engine, so it hears no votes.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PeerStatus {
    pub key: Vec<u8>,
    pub signed: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Query {
    pub layer: Layer,
    pub frame: Vec<u8>,
}

/// `host::Receipt`: what a submit answers.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Receipt {
    pub program: ProgramId,
    pub outcome: abi::Outcome,
    pub events: Vec<Vec<u8>>,
    /// The runs this one's messages caused, in order.
    pub nested: Vec<Receipt>,
}

/// One block's writes to one program, as `/v1/changes/<program>` streams them.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Change {
    pub height: u64,
    pub root: Root,
    pub writes: Vec<(Vec<u8>, Option<Vec<u8>>)>,
}

/// `wire::Blocks`: a page of finalized blocks, newest first.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Blocks {
    pub before: Option<u64>,
    pub limit: u32,
}

/// `wire::BlockRef`: a finalized block by height or by id.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BlockRef {
    Height(u64),
    Id([u8; 32]),
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Tx {
    pub hash: [u8; 32],
    pub signer: Vec<u8>,
    pub anchor: Tip,
    pub target: String,
    pub size: u32,
    pub payload: Option<Vec<u8>>,
    pub receipt: Option<Receipt>,
}

/// `wire::Finalized`: a block as the node's marshal archive keeps it.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Finalized {
    pub height: u64,
    pub id: [u8; 32],
    pub parent: [u8; 32],
    pub time: u64,
    pub epoch: u64,
    pub proposer: Option<Vec<u8>>,
    pub txs: Vec<Tx>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Page {
    pub blocks: Vec<Finalized>,
    pub listed_from: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum TxRef {
    At { height: u64, position: u32 },
    Hash([u8; 32]),
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TxAsk {
    pub tx: TxRef,
    pub most: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Located {
    pub height: u64,
    pub position: u32,
    pub time: u64,
    pub tx: Tx,
}

/// Unframes a blob the node handed over: `kind len\0body` → body.
pub fn unframe(framed: &[u8]) -> Option<&[u8]> {
    let nul = framed.iter().position(|byte| *byte == 0)?;
    Some(&framed[nul + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signature the node verifies a proof against is commonware's
    /// ed25519 over the namespaced preimage (core's `KeyScheme::Ed25519`
    /// verifies exactly that); ed25519 is deterministic, so the frame's
    /// proof is that signature byte for byte.
    #[cfg(feature = "commonware")]
    #[test]
    fn frame_signs_its_body_and_names_the_signer() {
        use commonware_cryptography::{Signer as Commonware, ed25519};
        let key = ed25519::PrivateKey::from_seed(7);
        let anchor = Tip {
            height: 3,
            id: [4; 32],
        };
        let frame = Frame::sign(&key, b"net", anchor, Some(5), "demo", vec![1, 2]);
        assert_eq!(frame.body.signer, Commonware::public_key(&key).as_ref());
        assert_eq!(frame.body.scheme, abi::Scheme::Ed25519);
        let decoded: Frame = abi::decode(&frame.encode()).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(frame.proof.len(), 64);
        let reference = Commonware::sign(&key, FRAME_NAMESPACE, &frame.body.preimage());
        assert_eq!(frame.proof, reference.as_ref());
    }

    #[test]
    fn a_query_frame_names_the_target_and_the_network_and_carries_no_salt() {
        let frame = Frame::query(&crate::FixedSigner, b"net", "demo", vec![1]).encode();
        let decoded: Frame = abi::decode(&frame).unwrap();
        assert_eq!(decoded.body.target, "demo");
        assert_eq!(decoded.body.network, b"net");
        assert_eq!(decoded.body.anchor, UNANCHORED);
        assert_eq!(decoded.body.salt, None);
    }

    #[test]
    fn a_body_encodes_in_the_nodes_field_order() {
        let body = Body {
            scheme: abi::Scheme::Ed25519,
            signer: vec![1],
            network: vec![2],
            anchor: Tip {
                height: 3,
                id: [4; 32],
            },
            salt: Some(5),
            target: "t".into(),
            payload: vec![6],
        };
        // core's `KeyScheme` tags: Ed25519 0, Secp256k1 1, Secp256r1 2
        assert_eq!(abi::encode(&abi::Scheme::Secp256k1), [1]);
        assert_eq!(abi::encode(&abi::Scheme::Secp256r1), [2]);
        let mut bytes = vec![0];
        bytes.extend([1, 0, 0, 0, 1, 1, 0, 0, 0, 2]);
        bytes.extend(3u64.to_le_bytes());
        bytes.extend([4; 32]);
        bytes.push(1);
        bytes.extend(5u64.to_le_bytes());
        bytes.extend([1, 0, 0, 0, b't', 1, 0, 0, 0, 6]);
        assert_eq!(body.preimage(), bytes);
    }

    #[test]
    fn a_tx_and_its_reads_follow_the_nodes_field_order() {
        let mut bytes = vec![9; 32];
        bytes.extend([1, 0, 0, 0, 1]);
        bytes.extend(3u64.to_le_bytes());
        bytes.extend([4; 32]);
        bytes.extend([1, 0, 0, 0, b't', 1, 0, 0, 0, 1, 1, 0, 0, 0, 6, 0]);
        let tx = Tx {
            hash: [9; 32],
            signer: vec![1],
            anchor: Tip {
                height: 3,
                id: [4; 32],
            },
            target: "t".into(),
            size: 1,
            payload: Some(vec![6]),
            receipt: None,
        };
        assert_eq!(abi::decode::<Tx>(&bytes).unwrap(), tx);
        let mut located = 7u64.to_le_bytes().to_vec();
        located.extend(2u32.to_le_bytes());
        located.extend(8u64.to_le_bytes());
        located.extend(&bytes);
        let at = Located {
            height: 7,
            position: 2,
            time: 8,
            tx,
        };
        assert_eq!(abi::decode::<Located>(&located).unwrap(), at);
        let mut ask = vec![0];
        ask.extend(7u64.to_le_bytes());
        ask.extend(2u32.to_le_bytes());
        ask.extend(9u32.to_le_bytes());
        let by_place = TxAsk {
            tx: TxRef::At {
                height: 7,
                position: 2,
            },
            most: 9,
        };
        assert_eq!(abi::encode(&by_place), ask);
        let page = Page {
            blocks: Vec::new(),
            listed_from: Some(3),
        };
        let mut listed = vec![0, 0, 0, 0, 1];
        listed.extend(3u64.to_le_bytes());
        assert_eq!(abi::decode::<Page>(&listed).unwrap(), page);
    }

    #[test]
    fn a_status_decodes_the_nodes_field_order_ending_in_the_roles() {
        let text = |text: &str| [&(text.len() as u32).to_le_bytes()[..], text.as_bytes()].concat();
        let mut bytes = text("n");
        for number in [1u64, 2, 3, 4] {
            bytes.extend(number.to_le_bytes());
        }
        bytes.extend([5; 32]);
        bytes.extend([6; 32]);
        bytes.extend(7u64.to_le_bytes());
        bytes.extend([1, 0, 0, 0, 8]);
        bytes.extend(NODE_CONTRACT.to_le_bytes());
        bytes.extend([9; 32]);
        for role in ["r", "v", "i"] {
            bytes.extend(text(role));
        }
        let status = Status {
            network: "n".into(),
            time: 1,
            block_time_ms: 2,
            epoch_length: 3,
            height: 4,
            tip: [5; 32],
            root: Root([6; 32]),
            epoch: 7,
            identity: vec![8],
            contract: NODE_CONTRACT,
            genesis: [9; 32],
            roles: abi::Roles {
                registry: "r".into(),
                validators: "v".into(),
                identity: "i".into(),
            },
        };
        assert_eq!(abi::decode::<Status>(&bytes).unwrap(), status);
    }

    /// The bytes `host::Receipt` encodes a submission with one nested run
    /// to: program "a", Applied [7], no events, nested [program "b",
    /// Applied [], no events, nested []].
    #[test]
    fn receipt_decodes_the_hosts_nested_runs() {
        let bytes = [
            1, 0, 0, 0, b'a', 0, 1, 0, 0, 0, 7, 0, 0, 0, 0, 1, 0, 0, 0, //
            1, 0, 0, 0, b'b', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let receipt: Receipt = abi::decode(&bytes).unwrap();
        assert_eq!(receipt.program, "a");
        assert_eq!(receipt.outcome, abi::Outcome::Applied { output: vec![7] });
        assert_eq!(receipt.nested.len(), 1);
        assert_eq!(receipt.nested[0].program, "b");
    }

    /// The bytes noded's own test pins `Network` to (noded/src/network.rs,
    /// `the_route_is_borsh_in_declaration_order`, core 4c12e1eac).
    #[test]
    fn network_decodes_the_nodes_bytes() {
        let mut bytes = Vec::new();
        bytes.extend(2u64.to_le_bytes());
        bytes.extend(2u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend([9, 1]);
        bytes.extend(1u64.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend([7, 0]);
        let network = Network {
            height: 2,
            members: vec![
                PeerStatus {
                    key: vec![9],
                    signed: Some(1),
                },
                PeerStatus {
                    key: vec![7],
                    signed: None,
                },
            ],
        };
        assert_eq!(abi::decode::<Network>(&bytes).unwrap(), network);
        assert_eq!(abi::encode(&network), bytes);
    }

    #[test]
    fn unframe_drops_the_git_header() {
        assert_eq!(unframe(b"blob 3\0abc"), Some(&b"abc"[..]));
        assert_eq!(unframe(b"no header"), None);
    }
}
