//! `program.describe`: an op as its program says a person reads it. The
//! program's CURRENT code blob (the roster the app already holds) carries a
//! wasm module in its `ducktape.describe` section; it is read out once per
//! code id (`::describe::section`, written so since this module
//! shadows the crate's name) and run by the view runner
//! (`ViewRunner::describe`: a fresh, import-free, fuel- and memory-bounded
//! instance), its answers kept per (code id, op hash).
//! The section is anyone's bytes: every failure is `None`, and the view
//! shows the op's bytes instead.
use std::collections::HashMap;
use std::sync::Arc;

use sha2::Digest as _;
use tokio::sync::OnceCell;

use super::node::{Answered, Node};
use super::*;
use crate::backend::views::{self, Fetch};

type Description = Option<methods::Description>;

/// One code id's describe module: its sha256, its bytes (small: kept, the
/// blob's map is not) and where they lie in the cache's file, for a
/// compile apart to read there; `None` where the code carries none. Set
/// once, by the first ask that fetched it, while every ask that came
/// meanwhile waits for it.
type Module = Option<([u8; 32], Arc<[u8]>, Option<(std::path::PathBuf, u64)>)>;
type Compiled = Arc<OnceCell<Module>>;

/// Describe modules by code id.
fn modules() -> &'static Mutex<HashMap<abi::BlobId, Compiled>> {
    static MODULES: OnceLock<Mutex<HashMap<abi::BlobId, Compiled>>> = OnceLock::new();
    MODULES.get_or_init(Mutex::default)
}

/// Answers by (code id, sha256 of the op).
type Answers = HashMap<(abi::BlobId, [u8; 32]), Description>;

fn answers() -> &'static Mutex<Answers> {
    static ANSWERS: OnceLock<Mutex<Answers>> = OnceLock::new();
    ANSWERS.get_or_init(Mutex::default)
}

pub(super) fn describe(node: Node, ask: Vec<u8>) -> Answered {
    Box::pin(async move {
        let (program, op): (String, Vec<u8>) = methods::decode(&ask).map_err(malformed)?;
        Ok(methods::encode(&described(&node, &program, op).await?))
    })
}

pub(super) async fn described(
    node: &Node,
    program: &str,
    op: Vec<u8>,
) -> Result<Description, wire::Error> {
    // a view-only entry is no program: it runs no ops
    let Some((code, false)) = node.roster().code(program) else {
        return Ok(None);
    };
    let key = (code, sha2::Sha256::digest(&op).into());
    if let Some(kept) = answers().lock().expect("describe answers").get(&key) {
        return Ok(kept.clone());
    }
    let Some((hash, module, file)) = module(node, code).await? else {
        return Ok(None);
    };
    let described = tokio::task::spawn_blocking(move || {
        let module = crate::contract::Code {
            bytes: &module,
            file: file.as_ref().map(|(path, at)| (path.as_path(), *at)),
        };
        crate::contract::runner().describe(hash, module, &op)
    })
    .await
    .ok()
    .flatten();
    answers()
        .lock()
        .expect("describe answers")
        .insert(key, described.clone());
    Ok(described)
}

/// The module in `code`'s describe section: fetched and read out once per
/// id, however many ask at once (a page of ops by one program
/// asks together). An ask that ends with nothing to keep (the node not
/// reached, the blob not held yet) sets nothing, and the next one tries.
async fn module(node: &Node, code: abi::BlobId) -> Result<Module, wire::Error> {
    let compiled = modules()
        .lock()
        .expect("describe modules")
        .entry(code)
        .or_default()
        .clone();
    let kept = compiled
        .get_or_try_init(|| async {
            let blob = match views::program_bytes(&node.client, &code).await {
                Ok((blob, _)) => blob,
                // the transport: the ask's answer, and the next ask tries again
                Err(Fetch::Unreachable(reason)) => {
                    return Err(Some(wire::Error::new(refusal::RPC_CLIENT, reason)));
                }
                // not held yet, or not the code asked for: nothing now, asked again later
                Err(_) => return Err(None),
            };
            Ok(tokio::task::spawn_blocking(move || {
                let section = blob.part(::describe::section)?;
                let code = section.code();
                let hash: [u8; 32] = sha2::Sha256::digest(code.bytes).into();
                let file = code.file.map(|(path, at)| (path.to_owned(), at));
                Some((hash, Arc::from(code.bytes), file))
            })
            .await
            .ok()
            .flatten())
        })
        .await;
    match kept {
        Ok(module) => Ok(module.clone()),
        Err(Some(refused)) => Err(refused),
        Err(None) => Ok(None),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn node() -> Node {
        Node {
            client: RpcClient::new("http://127.0.0.1:9"),
            network: "test#1".into(),
            apart: None,
        }
    }

    /// `program` listed at code `[seed; 32]`, whose describe module is `wat`
    /// (already compiled, so nothing is fetched).
    pub(in crate::runtime) fn list_program(program: &str, seed: u8, bare: bool, wat: Option<&str>) {
        let code = abi::BlobId::Sha256([seed; 32]);
        super::super::super::roster::roster()
            .lock()
            .push(crate::backend::views::Program {
                name: program.into(),
                code,
                bare,
            });
        let module = wat.map(|wat| {
            let bytes = wat::parse_str(wat).unwrap();
            let hash: [u8; 32] = sha2::Sha256::digest(&bytes).into();
            (hash, Arc::from(bytes), None)
        });
        modules().lock().unwrap().insert(code, kept(module));
    }

    /// A code id's entry, its module already read out (or known absent).
    fn kept(module: Module) -> Compiled {
        Arc::new(OnceCell::new_with(Some(module)))
    }

    /// Answers `Description { title: "hi", fields: [] }` for any op.
    pub(in crate::runtime) const HI: &str = r#"(module
        (memory (export "memory") 1)
        (data (i32.const 1024) "\02\00\00\00hi\00\00\00\00")
        (func (export "alloc") (param i32) (result i32) i32.const 0)
        (func (export "describe") (param i32 i32) (result i64) i64.const 0x4000000000a))"#;

    const LOOPS: &str = r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) i32.const 0)
        (func (export "describe") (param i32 i32) (result i64) (loop (br 0)) i64.const 0))"#;

    #[tokio::test]
    async fn an_op_reads_as_its_program_describes_it() {
        list_program("describe-good", 0xd1, false, Some(HI));
        let first = described(&node(), "describe-good", vec![1, 2]).await;
        assert_eq!(first.unwrap().unwrap().title, "hi");
        // kept per (code, op): the module is not entered again
        modules()
            .lock()
            .unwrap()
            .insert(abi::BlobId::Sha256([0xd1; 32]), kept(None));
        let kept = described(&node(), "describe-good", vec![1, 2]).await;
        assert_eq!(kept.unwrap().unwrap().title, "hi");
        assert_eq!(described(&node(), "describe-good", vec![3]).await, Ok(None));
    }

    #[tokio::test]
    async fn no_section_a_hostile_one_or_no_program_describes_nothing() {
        list_program("describe-loops", 0xd2, false, Some(LOOPS));
        list_program("describe-none", 0xd3, false, None);
        list_program("describe-view", 0xd4, true, Some(HI));
        for program in [
            "describe-loops",
            "describe-none",
            "describe-view",
            "unlisted",
        ] {
            assert_eq!(
                described(&node(), program, vec![1]).await,
                Ok(None),
                "{program}"
            );
        }
    }

    /// Asks for one code that arrive together share one fetch and one
    /// compile: the first does both, the rest wait for its module. Before,
    /// each ask that found no module fetched and compiled its own (the
    /// explorer's first Transactions page asked 38 at once).
    #[tokio::test]
    async fn asks_for_one_code_at_once_fetch_and_compile_it_once() {
        use super::super::tests::{Mode, fake_node};
        let section = wat::parse_str(HI).unwrap();
        let escaped: String = section.iter().map(|byte| format!("\\{byte:02x}")).collect();
        let program = wat::parse_str(format!(
            r#"(module (@custom "{}" "{escaped}"))"#,
            ::describe::SECTION
        ))
        .unwrap();
        let mut framed = format!("blob {}\0", program.len()).into_bytes();
        framed.extend_from_slice(&program);
        let code = abi::BlobId::Sha256(sha2::Sha256::digest(&framed).into());
        // the fetch keeps the blob in the developer's own cache: not there
        // before (it would answer instead of the node), gone after
        let cached = crate::backend::dirs()
            .cache
            .join("programs")
            .join(abi::hex(code.digest()));
        let _ = std::fs::remove_file(&cached);
        let node = fake_node(vec![(
            noded_client::wire::route::BLOB_GET,
            Mode::Answer(abi::encode(&Some(framed))),
        )]);
        let asks = futures::future::join_all((0..8).map(|_| module(&node.node, code))).await;
        let _ = std::fs::remove_file(&cached);
        let (_, first, _) = asks[0].clone().unwrap().expect("the section is read");
        assert!(
            asks.iter()
                .all(|ask| matches!(ask, Ok(Some((_, module, _))) if Arc::ptr_eq(module, &first))),
            "each ask read a section of its own"
        );
        assert_eq!(
            node.accepted.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "each ask fetched the code"
        );
    }

    /// An op is described by the code the asking request's roster lists: a
    /// followed set's own (another network's), never the app's, and the
    /// app's for a view.
    #[tokio::test]
    async fn describe_reads_the_requests_own_roster() {
        let program = "describe-apart";
        let code = abi::BlobId::Sha256([0xd5; 32]);
        let module = wat::parse_str(HI).unwrap();
        let hash: [u8; 32] = sha2::Sha256::digest(&module).into();
        modules()
            .lock()
            .unwrap()
            .insert(code, kept(Some((hash, Arc::from(module), None))));
        let apart = super::super::super::roster::Apart::default();
        apart.roster.lock().push(crate::backend::views::Program {
            name: program.into(),
            code,
            bare: false,
        });
        let followed = Node {
            apart: Some(Arc::new(apart)),
            ..node()
        };
        let read = described(&followed, program, vec![5]).await.unwrap();
        assert_eq!(read.map(|read| read.title).as_deref(), Some("hi"));
        assert_eq!(
            described(&node(), program, vec![5]).await,
            Ok(None),
            "the app's roster lists no such program"
        );
    }

    #[test]
    fn the_method_decodes_its_request_or_refuses_it() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let refused = runtime.block_on(describe(node(), vec![9])).unwrap_err();
        assert_eq!(refused.code, refusal::MALFORMED_REQUEST);
        let ask = methods::encode(&("unlisted".to_owned(), vec![1u8]));
        let answer = runtime.block_on(describe(node(), ask)).unwrap();
        assert_eq!(methods::decode::<Description>(&answer), Ok(None));
    }
}

/// The qa check: every op in a running stage's archive, through this method's
/// own path (roster, code blob, section, module), and none of the programs
/// that ship a describe module falls back to bytes. Against a stage:
/// `DESCRIBE_STAGE=http://<listen> cargo test --release every_op_on_a_stage -- --ignored --nocapture`
#[cfg(test)]
mod stage {
    use super::*;
    use noded_client::wire::Blocks;

    #[tokio::test]
    #[ignore = "reads a running stage: DESCRIBE_STAGE=http://<listen>"]
    async fn every_op_on_a_stage_is_described() {
        let endpoint = std::env::var("DESCRIBE_STAGE").expect("DESCRIBE_STAGE names the node");
        let client = RpcClient::new(endpoint);
        let network = client.status().await.expect("the stage answers").network;
        let programs = views::programs(&client, &network, &*crate::backend::query_signer())
            .await
            .expect("its roster");
        *super::super::super::roster::roster().lock() = programs;
        let node = Node {
            client,
            network,
            apart: None,
        };
        let mut counts: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
        let every = Blocks {
            before: None,
            limit: u32::MAX,
            most: u32::MAX,
        };
        let chain = node.client.blocks(&every).await.expect("the blocks");
        for tx in chain.blocks.into_iter().flat_map(|block| block.txs) {
            let payload = tx.payload.expect("its payload");
            let described = described(&node, &tx.target, payload).await.unwrap();
            let count = counts.entry(tx.target).or_default();
            match described {
                Some(_) => count.0 += 1,
                None => count.1 += 1,
            }
        }
        for (program, (described, bytes)) in &counts {
            println!("{program}: {described} described, {bytes} as bytes");
        }
        let ours = ["chat", "forge", "identity", "valset", "programs"];
        let fell_back: Vec<_> = counts
            .iter()
            .filter(|(program, (_, bytes))| ours.contains(&program.as_str()) && *bytes > 0)
            .collect();
        assert!(!counts.is_empty(), "the stage holds no ops");
        assert!(fell_back.is_empty(), "fell back to bytes: {fell_back:?}");
    }
}
