//! `ducktape-app send <network> <program> <op>`: one transaction from the
//! terminal to a network this app has connected to, signed with this
//! device's key for it. `main.rs` dispatches it; the README says how to use it.

use std::fmt;
use std::sync::Arc;

use borsh_json::{Fault, Interface, SECTION, Unreadable};
use noded_client::hex::{hex_decode, hex_encode};
use noded_client::wire::{NODE_CONTRACT, Receipt, Status, TxAsk, TxRef};
use serde_json::Value;

use super::views::Fetch;
use super::{RecentEndpoint, RpcClient};

pub(crate) const USAGE: &str =
    "usage: ducktape-app send <network> <program> <op as JSON, hex or base64>";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    pub(crate) network: String,
    pub(crate) program: String,
    pub(crate) call: Call,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Call {
    Json(Value),
    Bytes(Vec<u8>),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Wrong {
    Count,
    Json(String),
    Unread,
}

impl fmt::Display for Wrong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Wrong::Count => f.write_str("it takes three arguments"),
            Wrong::Json(error) => write!(f, "the op starts as JSON and does not parse ({error})"),
            Wrong::Unread => f.write_str("the op is neither JSON, hex nor base64"),
        }
    }
}

impl Ask {
    pub(crate) fn parse(args: &[String]) -> Result<Ask, Wrong> {
        let [network, program, op] = args else {
            return Err(Wrong::Count);
        };
        Ok(Ask {
            network: network.clone(),
            program: program.clone(),
            call: Call::read(op)?,
        })
    }
}

impl Call {
    pub(crate) fn read(given: &str) -> Result<Call, Wrong> {
        let given = given.trim();
        if given.starts_with(['{', '"']) {
            let parsed = serde_json::from_str(given);
            return parsed
                .map(Call::Json)
                .map_err(|error| Wrong::Json(error.to_string()));
        }
        let digits = given.strip_prefix("0x").unwrap_or(given);
        if let Ok(bytes) = hex_decode(digits) {
            return Ok(Call::Bytes(bytes));
        }
        base64_read(given).map(Call::Bytes).ok_or(Wrong::Unread)
    }
}

fn base64_read(given: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
    let bare = given.trim_end_matches('=');
    let standard = STANDARD_NO_PAD.decode(bare);
    standard.or_else(|_| URL_SAFE_NO_PAD.decode(bare)).ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Bound {
    pub(crate) network: String,
    pub(crate) founded: u64,
    pub(crate) url: String,
}

impl Bound {
    fn full(&self) -> String {
        format!("{}+{}", self.network, self.founded)
    }
}

pub(crate) fn bound(recent: &[RecentEndpoint]) -> Vec<Bound> {
    let mut chains: Vec<Bound> = Vec::new();
    for entry in recent {
        let known = chains
            .iter()
            .any(|chain| chain.network == entry.network && chain.founded == entry.founded);
        if known {
            continue;
        }
        chains.push(Bound {
            network: entry.network.clone(),
            founded: entry.founded,
            url: entry.url.clone(),
        });
    }
    chains
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) network: String,
    pub(crate) founded: Option<u64>,
    pub(crate) url: String,
}

impl Target {
    fn runs(&self, status: &Status) -> bool {
        let named = status.network == self.network;
        let founded = self.founded.is_none_or(|founded| founded == status.time);
        named && founded
    }
}

pub(crate) fn target(bound: &[Bound], asked: &str) -> Option<Target> {
    if let Some(chain) = bound.iter().find(|chain| chain.full() == asked) {
        return Some(Target {
            network: chain.network.clone(),
            founded: Some(chain.founded),
            url: chain.url.clone(),
        });
    }
    bound
        .iter()
        .find(|chain| chain.network == asked)
        .map(|chain| Target {
            network: chain.network.clone(),
            founded: None,
            url: chain.url.clone(),
        })
}

#[derive(Debug)]
pub(crate) enum Failure {
    Runtime(std::io::Error),
    Unbound(String),
    Unreached {
        url: String,
        error: noded_client::Error,
    },
    Contract {
        url: String,
        node: u32,
    },
    Elsewhere {
        url: String,
        runs: String,
    },
    Keyring(String),
    NoKey {
        url: String,
        chain: String,
    },
    Key(String),
    Password(std::io::Error),
    Unrostered(Fetch),
    Unlisted(String),
    Uncoded {
        program: String,
        fetch: Fetch,
    },
    Schemaless(String),
    Unreadable {
        program: String,
        why: Unreadable,
    },
    Unencoded {
        program: String,
        fault: Fault,
    },
    Unsigned(view_wire::Error),
    Unsent(noded_client::Error),
    Refused(abi::Refusal),
    InDoubt {
        frame: [u8; 32],
        until: u64,
        error: noded_client::Error,
    },
    Unlanded {
        frame: [u8; 32],
        until: u64,
    },
    Unfollowed {
        frame: [u8; 32],
        error: noded_client::Error,
    },
}

impl Failure {
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Failure::Unbound(_) => 2,
            _ => 1,
        }
    }

    fn names_the_bound(&self) -> bool {
        matches!(
            self,
            Failure::Unbound(_)
                | Failure::Unreached { .. }
                | Failure::Elsewhere { .. }
                | Failure::NoKey { .. }
        )
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Runtime(error) => write!(f, "no async runtime could start: {error}"),
            Failure::Unbound(asked) => {
                write!(f, "no network called {asked} is bound on this device")
            }
            Failure::Unreached { url, error } => {
                write!(f, "tried {url}, which did not answer: {error}")
            }
            Failure::Contract { url, node } => write!(
                f,
                "{url} speaks contract {node}; this app speaks {NODE_CONTRACT}"
            ),
            Failure::Elsewhere { url, runs } => write!(f, "tried {url}, which now runs {runs}"),
            Failure::Keyring(sentence) | Failure::Key(sentence) => f.write_str(sentence),
            Failure::NoKey { url, chain } => write!(
                f,
                "tried {url}, which runs {chain}; this device holds no key for that chain"
            ),
            Failure::Password(error) => write!(f, "the password could not be read: {error}"),
            Failure::Unrostered(fetch) => {
                write!(f, "the programs role's roster could not be read: {fetch}")
            }
            Failure::Unlisted(program) => {
                write!(f, "the network runs no program called {program}")
            }
            Failure::Uncoded { program, fetch } => {
                write!(f, "the code of {program} could not be read: {fetch}")
            }
            Failure::Schemaless(program) => write!(
                f,
                "the code of {program} carries no {SECTION} section, so no JSON encodes for it; send its bytes as hex or base64"
            ),
            Failure::Unreadable { program, why } => {
                write!(f, "the {SECTION} section of {program}'s code {why}")
            }
            Failure::Unencoded { program, fault } => {
                write!(f, "the op does not fit {program}'s schema: {fault}")
            }
            Failure::Unsigned(error) => write!(f, "{}: {}", error.code, error.message),
            Failure::Unsent(error) => write!(f, "the frame did not reach the node: {error}"),
            Failure::Refused(refusal) => {
                write!(f, "refused {}: {}", refusal.reason, refusal.sentence)
            }
            Failure::InDoubt {
                frame,
                until,
                error,
            } => write!(
                f,
                "the node's answer to frame {} went missing ({error}); it may still land by block {until}",
                hex_encode(frame)
            ),
            Failure::Unlanded { frame, until } => write!(
                f,
                "no block carried frame {} by block {until}, the last that may",
                hex_encode(frame)
            ),
            Failure::Unfollowed { frame, error } => write!(
                f,
                "frame {} was preconfirmed, but the block that carries it could not be read: {error}",
                hex_encode(frame)
            ),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Sent {
    pub(crate) frame: [u8; 32],
    pub(crate) height: Option<u64>,
    pub(crate) outcome: abi::Outcome,
}

impl Sent {
    pub(crate) fn exit_code(&self) -> i32 {
        match self.outcome {
            abi::Outcome::Applied { .. } => 0,
            abi::Outcome::Rejected(_) => 1,
        }
    }
}

impl fmt::Display for Sent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "frame {}", hex_encode(&self.frame))?;
        if let Some(height) = self.height {
            writeln!(f, "height {height}")?;
        }
        match &self.outcome {
            abi::Outcome::Applied { output } if output.is_empty() => writeln!(f, "applied"),
            abi::Outcome::Applied { output } => {
                writeln!(f, "applied")?;
                writeln!(f, "output {}", hex_encode(output))
            }
            abi::Outcome::Rejected(refusal) => {
                writeln!(f, "refused {}: {}", refusal.reason, refusal.sentence)
            }
        }
    }
}

pub(crate) fn cli(args: &[String], install: impl FnOnce()) -> i32 {
    let ask = match Ask::parse(args) {
        Ok(ask) => ask,
        Err(wrong) => {
            eprintln!("ducktape-app send: {wrong}");
            eprintln!("{USAGE}");
            return 2;
        }
    };
    install();
    match send(&ask) {
        Ok(sent) => {
            print!("{sent}");
            sent.exit_code()
        }
        Err(failure) => {
            eprintln!("ducktape-app send: {failure}");
            if failure.names_the_bound() {
                eprint!("{}", BoundList(bound(&super::recent_endpoints())));
            }
            failure.exit_code()
        }
    }
}

struct BoundList(Vec<Bound>);

impl fmt::Display for BoundList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return writeln!(
                f,
                "no network is bound on this device: connect to one in the app first"
            );
        }
        writeln!(
            f,
            "bound networks (a bare name reaches the node the app last used for it):"
        )?;
        for chain in &self.0 {
            writeln!(f, "  {}  {}", chain.full(), chain.url)?;
        }
        Ok(())
    }
}

pub(crate) fn send(ask: &Ask) -> Result<Sent, Failure> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Failure::Runtime)?;
    let target = target(&bound(&super::recent_endpoints()), &ask.network)
        .ok_or_else(|| Failure::Unbound(ask.network.clone()))?;
    let client = super::rpc_client(target.url.clone());
    let status = runtime.block_on(reach(&client, &target))?;
    let keyring = noded_client::keyring::bind(
        ducktape_host::contract::os().disk(),
        &super::dirs().remotes,
        &status.network,
        status.time,
    )
    .map_err(Failure::Keyring)?;
    let key = open_key(&keyring.dir, &target.url, &status)?;
    runtime.block_on(submit(&client, &status, key, &ask.program, &ask.call))
}

async fn reach(client: &RpcClient, target: &Target) -> Result<Status, Failure> {
    let url = target.url.clone();
    let status = match client.status().await {
        Ok(status) => status,
        Err(error) => return Err(Failure::Unreached { url, error }),
    };
    if status.contract != NODE_CONTRACT {
        return Err(Failure::Contract {
            url,
            node: status.contract,
        });
    }
    if !target.runs(&status) {
        let runs = format!("{}+{}", status.network, status.time);
        return Err(Failure::Elsewhere { url, runs });
    }
    Ok(status)
}

fn open_key(
    keyring: &str,
    url: &str,
    status: &Status,
) -> Result<Arc<dyn noded_client::Signer>, Failure> {
    let kept = super::device_key::open_kept(keyring).map_err(Failure::Key)?;
    let password_locked = super::key_exists(keyring);
    match (kept, password_locked) {
        (Some(key), _) => Ok(Arc::new(key)),
        (None, true) => unlock(keyring),
        (None, false) => Err(Failure::NoKey {
            url: url.to_owned(),
            chain: format!("{}+{}", status.network, status.time),
        }),
    }
}

fn unlock(keyring: &str) -> Result<Arc<dyn noded_client::Signer>, Failure> {
    let password = zeroize::Zeroizing::new(
        rpassword::prompt_password("This device's key is locked with a password. Password: ")
            .map_err(Failure::Password)?,
    );
    let key = super::open_password_locked(keyring, &password)
        .map_err(|error| Failure::Key(super::user_error(error)))?;
    Ok(Arc::new(key))
}

async fn encoded(
    client: &RpcClient,
    status: &Status,
    signer: &dyn noded_client::Signer,
    program: &str,
    op: &Value,
) -> Result<Vec<u8>, Failure> {
    let roster = super::views::programs(client, &status.network, signer)
        .await
        .map_err(Failure::Unrostered)?;
    let listed = roster
        .iter()
        .find(|entry| entry.name == program && !entry.bare)
        .ok_or_else(|| Failure::Unlisted(program.to_owned()))?;
    let (code, _) = super::views::program_bytes(client, &listed.code)
        .await
        .map_err(|fetch| Failure::Uncoded {
            program: program.to_owned(),
            fetch,
        })?;
    let interface = interface_of(program, &code)?;
    interface.encode_op(op).map_err(|fault| Failure::Unencoded {
        program: program.to_owned(),
        fault,
    })
}

pub(crate) fn interface_of(program: &str, code: &[u8]) -> Result<Interface, Failure> {
    let section = super::views::section(code, SECTION)
        .ok_or_else(|| Failure::Schemaless(program.to_owned()))?;
    Interface::read(section).map_err(|why| Failure::Unreadable {
        program: program.to_owned(),
        why,
    })
}

async fn submit(
    client: &RpcClient,
    status: &Status,
    key: Arc<dyn noded_client::Signer>,
    program: &str,
    call: &Call,
) -> Result<Sent, Failure> {
    super::seat_key(&status.network, key.clone()).await;
    let payload = match call {
        Call::Bytes(bytes) => bytes.clone(),
        Call::Json(op) => encoded(client, status, &*key, program, op).await?,
    };
    let frame = super::seated_frame(client, &status.network, program, payload)
        .await
        .map_err(Failure::Unsigned)?;
    let (id, until) = (frame.body.id(), frame.body.until());
    let receipt = match client.preconfirm(frame.encode()).await {
        Ok(receipt) => receipt,
        Err(noded_client::Error::Refused(refusal)) => return Err(Failure::Refused(refusal)),
        Err(error @ noded_client::Error::Unreachable(_)) => return Err(Failure::Unsent(error)),
        Err(
            error @ (noded_client::Error::Transport(_)
            | noded_client::Error::Failed { .. }
            | noded_client::Error::Decode(_)),
        ) => {
            return Err(Failure::InDoubt {
                frame: id,
                until,
                error,
            });
        }
    };
    match receipt.outcome {
        abi::Outcome::Rejected(_) => Ok(Sent {
            frame: id,
            height: None,
            outcome: receipt.outcome,
        }),
        abi::Outcome::Applied { .. } => {
            let pace = std::time::Duration::from_millis(status.block_time_ms / 2);
            landed(client, id, until, pace, receipt).await
        }
    }
}

async fn landed(
    client: &RpcClient,
    frame: [u8; 32],
    until: u64,
    pace: std::time::Duration,
    preconfirmed: Receipt,
) -> Result<Sent, Failure> {
    let ask = TxAsk {
        tx: TxRef::Hash(frame),
        most: 0,
    };
    let unfollowed = |error| Failure::Unfollowed { frame, error };
    loop {
        let tip = client.status().await.map_err(unfollowed)?.height;
        let Some(located) = client.tx(&ask).await.map_err(unfollowed)? else {
            if tip >= until {
                return Err(Failure::Unlanded { frame, until });
            }
            tokio::time::sleep(pace).await;
            continue;
        };
        let receipt = located.tx.receipt.unwrap_or(preconfirmed);
        return Ok(Sent {
            frame,
            height: Some(located.height),
            outcome: receipt.outcome,
        });
    }
}

#[cfg(test)]
mod tests;
