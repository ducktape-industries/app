//! `module.describe`: an op as its program says a person reads it. The
//! program's CURRENT code blob (the roster the app already holds) carries a
//! wasm module in its `ducktape.describe` section; it is compiled once per
//! code id and called in a fresh, import-free, fuel- and memory-bounded
//! instance (the `describe` crate's `host`, written `::describe::host` here
//! since this module shadows the crate's name), its answers kept per (code
//! id, op hash).
//! The section is anyone's bytes: every failure is `None`, and the view
//! shows the op's bytes instead.
use std::collections::HashMap;
use std::sync::Arc;

use sha2::Digest as _;
use tokio::sync::OnceCell;
use wasmtime::Module;

use super::node::{Answered, Node};
use super::*;
use crate::backend::views::{self, Fetch};

type Description = Option<methods::Description>;

/// Answers kept before the table starts over.
const MAX_KEPT: usize = 4096;

/// One code id's compiled module, `None` where the code carries none: set
/// once, by the first ask that fetched and compiled it, while every ask
/// that came meanwhile waits for it.
type Compiled = Arc<OnceCell<Option<Arc<Module>>>>;

/// Compiled modules by code id.
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

async fn described(node: &Node, program: &str, op: Vec<u8>) -> Result<Description, wire::Error> {
    // a view-only entry is no program: it runs no ops
    let Some((code, false)) = super::super::roster::roster().code(program) else {
        return Ok(None);
    };
    let key = (code, sha2::Sha256::digest(&op).into());
    if let Some(kept) = answers().lock().expect("describe answers").get(&key) {
        return Ok(kept.clone());
    }
    let Some(module) = module(node, code).await? else {
        return Ok(None);
    };
    let described = tokio::task::spawn_blocking(move || {
        ::describe::host::run(super::super::guest::engine(), &module, &op)
    })
    .await
    .ok()
    .flatten();
    let mut kept = answers().lock().expect("describe answers");
    // ponytail: starts over when full; an LRU if a window ever outgrows it
    if kept.len() >= MAX_KEPT {
        kept.clear();
    }
    kept.insert(key, described.clone());
    Ok(described)
}

/// The module in `code`'s describe section, compiled: fetched and compiled
/// once per id, however many ask at once (a page of ops by one program
/// asks together). An ask that ends with nothing to keep (the node not
/// reached, the blob not held yet) sets nothing, and the next one tries.
async fn module(node: &Node, code: abi::BlobId) -> Result<Option<Arc<Module>>, wire::Error> {
    let compiled = modules()
        .lock()
        .expect("describe modules")
        .entry(code)
        .or_default()
        .clone();
    let kept = compiled
        .get_or_try_init(|| async {
            let bytes = match views::program_bytes(&node.client, &code).await {
                Ok((bytes, _)) => bytes,
                // the transport: retried by the node method's loop
                Err(Fetch::Unreachable(reason)) => {
                    return Err(Some(wire::Error::new(refusal::RPC_CLIENT, reason)));
                }
                // not held yet, or not the code asked for: nothing now, asked again later
                Err(_) => return Err(None),
            };
            Ok(tokio::task::spawn_blocking(move || {
                ::describe::host::section(&bytes)
                    .and_then(|section| {
                        ::describe::host::compile(super::super::guest::engine(), section)
                    })
                    .map(Arc::new)
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
mod tests {
    use super::*;

    fn node() -> Node {
        Node {
            client: RpcClient::new("http://127.0.0.1:9"),
            network: "test#1".into(),
        }
    }

    /// `program` listed at code `[seed; 32]`, whose describe module is `wat`
    /// (already compiled, so nothing is fetched).
    fn list_program(program: &str, seed: u8, bare: bool, wat: Option<&str>) {
        let code = abi::BlobId::Sha256([seed; 32]);
        super::super::super::roster::roster()
            .lock()
            .push(crate::backend::views::Program {
                name: program.into(),
                code,
                bare,
            });
        let module = wat.and_then(|wat| {
            ::describe::host::compile(
                super::super::super::guest::engine(),
                &wat::parse_str(wat).unwrap(),
            )
        });
        modules()
            .lock()
            .unwrap()
            .insert(code, kept(module.map(Arc::new)));
    }

    /// A code id's entry, its module already compiled (or known absent).
    fn kept(module: Option<Arc<Module>>) -> Compiled {
        Arc::new(OnceCell::new_with(Some(module)))
    }

    /// Answers `Description { title: "hi", fields: [] }` for any op.
    const HI: &str = r#"(module
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
        let cached = crate::backend::cache_dir()
            .unwrap()
            .join("programs")
            .join(abi::hex(code.digest()));
        let _ = std::fs::remove_file(&cached);
        let node = fake_node(vec![(
            crate::backend::noded::route::BLOB_GET,
            Mode::Answer(abi::encode(&Some(framed))),
        )]);
        let asks = futures::future::join_all((0..8).map(|_| module(&node.node, code))).await;
        let _ = std::fs::remove_file(&cached);
        let first = asks[0].clone().unwrap().expect("the section compiles");
        assert!(
            asks.iter()
                .all(|ask| matches!(ask, Ok(Some(module)) if Arc::ptr_eq(module, &first))),
            "each ask compiled a module of its own"
        );
        assert_eq!(
            node.accepted.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "each ask fetched the code"
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
    use crate::backend::noded::Blocks;

    #[tokio::test]
    #[ignore = "reads a running stage: DESCRIBE_STAGE=http://<listen>"]
    async fn every_op_on_a_stage_is_described() {
        let endpoint = std::env::var("DESCRIBE_STAGE").expect("DESCRIBE_STAGE names the node");
        let client = RpcClient::new(endpoint);
        let network = client.status().await.expect("the stage answers").network;
        let programs = views::programs(&client, &network)
            .await
            .expect("its roster");
        *super::super::super::roster::roster().lock() = programs;
        let node = Node { client, network };
        let mut counts: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
        let mut before = None;
        loop {
            let page = node
                .client
                .blocks(&Blocks { before, limit: 100 })
                .await
                .expect("a page of blocks");
            let Some(last) = page.last().map(|block| block.height) else {
                break;
            };
            for tx in page.into_iter().flat_map(|block| block.txs) {
                let described = described(&node, &tx.target, tx.payload).await.unwrap();
                let count = counts.entry(tx.target).or_default();
                match described {
                    Some(_) => count.0 += 1,
                    None => count.1 += 1,
                }
            }
            if last == 0 {
                break;
            }
            before = Some(last);
        }
        for (program, (described, bytes)) in &counts {
            println!("{program}: {described} described, {bytes} as bytes");
        }
        let ours = ["chat", "forge", "identity", "valset", "module-registry"];
        let fell_back: Vec<_> = counts
            .iter()
            .filter(|(program, (_, bytes))| ours.contains(&program.as_str()) && *bytes > 0)
            .collect();
        assert!(!counts.is_empty(), "the stage holds no ops");
        assert!(fell_back.is_empty(), "fell back to bytes: {fell_back:?}");
    }
}
