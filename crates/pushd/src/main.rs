//! pushd, the app's host without a window.
//!
//! What becomes a notice, for whom, its words, its tag, its link and when it
//! is taken down are decided by EACH PROGRAM in its own code: its notifier,
//! the `ducktape.notify` section of its code blob. The SDK fixes the shape;
//! pushd, like the app, is a generic carrier. It names no program and judges
//! no notice.
//!
//! For each network it follows ONE node and trusts it, as the app does: no
//! light client, no block sync. For each account with a device in the
//! registry (`registry.rs`: a phone registers its push token signed by a
//! key identity says acts as the account), it runs every listed program's
//! notifier through the app's own runtime
//! (`ducktape_host::runtime::notify_for`), as the desktop app runs it for the
//! person at the screen: the same kernel contract, the same store, the same
//! notification centre. Per account it is handed a session of its own
//! (`signer` the account's canonical key, `connected`, no screen), a store
//! under `--data`, a centre (the fold by tag, the burst limit) and a channel.
//! A notifier never signs (`op.submit` is not a notifier's), so pushd holds
//! no user key.
//!
//! The channel is `push` (`push.rs`): a call goes to the account's phones as
//! an APNs VoIP push and an FCM high-priority message, a notice as an APNs
//! alert and an FCM message the app shows, a withdraw as a background push
//! and a `withdraw` message; every notice is also one JSON line on stdout
//! (`log`), as is each badge that moved. The APNs auth key and the Firebase
//! service account are files pushd.toml names (`[apns]`, `[fcm]`), never in
//! a repo; without them a notice is logged only.
//!
//! The runtime keeps one network per process, so pushd runs one process per
//! network: with several in the config and no `--network`, it starts itself
//! once for each and waits on them. The registry's door (`listen`) is the
//! process without `--network`: the supervisor, or the one process of a
//! one-network config. The children read the registry's files.
//!
//! ```text
//! pushd --config pushd.toml --data /var/lib/pushd [--network NAME]
//! pushd probe --config pushd.toml      one push per credential to a made-up token: the answer proves the credential
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ducktape_host::backend;
use ducktape_host::contract::{
    self, Banners, Dirs, KeyRefusal, KeyStorage, Limits, MintInvite, OsFacts, OsServices, Preload,
};
use ducktape_host::runtime::notify::{
    CenterHandle, Click, Notice, NoticeKind, Permission, Settings,
};
use ducktape_host::runtime::{self, Answered, Carried, Intent, Recipient};

mod push;
mod registry;
use push::{Apns, ApnsConfig, Fcm, FcmConfig, Push};
use registry::{Devices, Door, Followed};

/// How often the node's status is read: the app's own pace
/// (`session::node::STATUS_EVERY`). A moved height reads the roster again;
/// the registry's file is looked at too.
const STATUS_EVERY: Duration = Duration::from_secs(2);

/// Banners a minute per program, past which one "N more" stands for the
/// rest: the app's default.
const BURST: u32 = 6;

/// Each network, the node pushd follows on it, the door devices register
/// through, and the credentials the pushes are sent with.
#[derive(Debug, serde::Deserialize)]
struct Config {
    /// None at all: a config `probe` reads, with the credentials only.
    #[serde(default)]
    networks: Vec<Network>,
    /// Where the registry's door listens (`127.0.0.1:9201` behind the
    /// tunnel); none: no device can register, the ones registered stay.
    listen: Option<String>,
    /// The team's APNs auth key: pushes to an iPhone.
    apns: Option<ApnsConfig>,
    /// The Firebase project's service account: pushes to an Android phone.
    fcm: Option<FcmConfig>,
}

#[derive(Debug, serde::Deserialize)]
struct Network {
    /// A name for the network here: its directory under `--data`.
    name: String,
    node_url: String,
}

/// Where a network's registered devices are kept.
fn devices_file(data: &Path, network: &str) -> PathBuf {
    data.join(network).join("devices.json")
}

fn main() -> ExitCode {
    // a notifier's compile, in a child so its memory leaves with it
    let mut args = std::env::args_os().skip(1);
    if args
        .next()
        .is_some_and(|arg| arg == ducktape_host::runner::COMPILE_APART)
    {
        let args: Vec<std::ffi::OsString> = args.collect();
        std::process::exit(ducktape_host::runner::compile_child(&args));
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    match start() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(target: "pushd", %error, "pushd stopped");
            ExitCode::FAILURE
        }
    }
}

fn start() -> Result<(), String> {
    let mut args = std::env::args().skip(1).peekable();
    let probing = args.next_if_eq("probe").is_some();
    let (mut config, mut data, mut only) = (None, None, None);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} takes a value"))?;
        match arg.as_str() {
            "--config" => config = Some(PathBuf::from(value)),
            "--data" => data = Some(PathBuf::from(value)),
            "--network" => only = Some(value),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let usage = "pushd --config FILE --data DIR [--network NAME] | pushd probe --config FILE";
    let config_path = config.ok_or(usage)?;
    let text = std::fs::read_to_string(&config_path)
        .map_err(|error| format!("{}: {error}", config_path.display()))?;
    let config: Config = toml::from_str(&text).map_err(|error| error.to_string())?;
    if probing {
        return probe(&config);
    }
    let data = data.ok_or(usage)?;
    if let Some(bad) = config.networks.iter().find(|network| !named(&network.name)) {
        return Err(format!(
            "a network's name is [A-Za-z0-9._-]: {:?}",
            bad.name
        ));
    }
    let web = |url: &str| url.starts_with("http://") || url.starts_with("https://");
    if let Some(bad) = config
        .networks
        .iter()
        .find(|network| !web(&network.node_url))
    {
        return Err(format!(
            "a network's node_url is an http:// or https:// URL: {:?}",
            bad.node_url
        ));
    }
    match (only, &config.networks[..]) {
        (_, []) => Err("the config lists no [[networks]]".into()),
        (Some(name), networks) => {
            let network = networks
                .iter()
                .find(|network| network.name == name)
                .ok_or_else(|| format!("the config lists no network {name}"))?;
            run(network, &config, &data)
        }
        (None, [network]) => {
            open_door(&config, &data)?;
            run(network, &config, &data)
        }
        (None, networks) => {
            open_door(&config, &data)?;
            supervise(&config_path, &data, networks)
        }
    }
}

/// A name that is one path segment.
fn named(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// The registry's door on `listen`, for every network of the config, when
/// the config names one.
fn open_door(config: &Config, data: &Path) -> Result<(), String> {
    let Some(listen) = &config.listen else {
        tracing::warn!(target: "pushd", "no `listen`: no device can register");
        return Ok(());
    };
    let listener =
        std::net::TcpListener::bind(listen).map_err(|error| format!("{listen}: {error}"))?;
    let networks = config
        .networks
        .iter()
        .map(|network| {
            Ok(Arc::new(Followed {
                name: network.name.clone(),
                devices: Devices::load(&devices_file(data, &network.name))?,
                client: backend::rpc_client(network.node_url.clone()),
                chain: Mutex::default(),
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    tracing::info!(target: "pushd", listen, networks = networks.len(), "the door is open");
    Arc::new(Door::new(networks, Arc::new(reader_key()))).serve(listener)
}

/// One pushd per network, each this binary with `--network`: until the
/// first one ends, which ends them all (a supervisor such as systemd starts
/// them again).
fn supervise(config: &Path, data: &Path, networks: &[Network]) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut children = Vec::new();
    for network in networks {
        let child = std::process::Command::new(&me)
            .arg("--config")
            .arg(config)
            .arg("--data")
            .arg(data)
            .args(["--network", &network.name])
            .spawn()
            .map_err(|error| format!("{}: {error}", network.name))?;
        children.push((network.name.clone(), child));
    }
    loop {
        for at in 0..children.len() {
            let ended = children[at]
                .1
                .try_wait()
                .map_err(|error| error.to_string())?;
            if let Some(status) = ended {
                for (_, other) in &mut children {
                    let _ = other.kill();
                }
                return Err(format!("the pushd of {} ended: {status}", children[at].0));
            }
        }
        std::thread::sleep(STATUS_EVERY);
    }
}

/// The credentials loaded, as `run` and `probe` take them.
type Credentials = (Option<Arc<Apns>>, Option<Arc<Fcm>>);

fn credentials(config: &Config) -> Result<Credentials, String> {
    let apns = config
        .apns
        .as_ref()
        .map(Apns::load)
        .transpose()?
        .map(Arc::new);
    let fcm = config
        .fcm
        .as_ref()
        .map(Fcm::load)
        .transpose()?
        .map(Arc::new);
    Ok((apns, fcm))
}

/// pushd on one network: the host installed, the node connected, and its
/// status and the registry followed for as long as it runs: one recipient
/// per account with a device, on the account's canonical signer.
fn run(network: &Network, config: &Config, data: &Path) -> Result<(), String> {
    let (apns, fcm) = credentials(config)?;
    let http: Arc<dyn push::Http> = Arc::new(push::Client::new()?);
    let home = data.join(&network.name);
    contract::install(Arc::new(Headless::new(&home)?));
    // the notifiers run on wasmtime, their compiled code kept across runs
    // and compiled in a child of this binary, as the desktop's views are:
    // a compile's peak, on wasmtime's thread per core, leaves with it
    contract::install_runner(Arc::new(ducktape_host::runner::WasmtimeRunner::new(
        contract::RunnerConfig {
            signals_based_traps: true,
            cache_dir: Some(home.join("cache").join("view-code")),
            // Linux: the running binary even once a deploy replaced its file
            compile_apart: match cfg!(target_os = "linux") {
                true => Some("/proc/self/exe".into()),
                false => std::env::current_exe().ok(),
            },
        },
    )));
    let client = backend::rpc_client(network.node_url.clone());
    let status = loop {
        match runtime::handle().block_on(client.status()) {
            Ok(status) => break status,
            Err(error) => {
                tracing::warn!(target: "pushd", node = network.node_url, %error, "node not reached");
                std::thread::sleep(STATUS_EVERY);
            }
        }
    };
    // the chain id `duck://` links name, as the app spells it
    let chain = ducklink::ChainId::of(&status.network, &status.genesis)
        .map_or_else(|| status.network.clone(), |chain| chain.to_string());
    let file = devices_file(data, &network.name);
    let devices = Devices::load(&file)?;
    tracing::info!(target: "pushd", network = network.name, chain, height = status.height,
        accounts = devices.accounts().len(), "following the node");
    let mut carried: HashMap<u64, (String, Carried)> = HashMap::new();
    let carry = |carried: &mut HashMap<u64, (String, Carried)>| {
        let wanted = devices.accounts();
        carried.retain(|account, (signer, _)| wanted.contains(&(*account, signer.clone())));
        for (account, signer) in wanted {
            if carried.contains_key(&account) {
                continue;
            }
            let (said, heard) = futures::channel::mpsc::unbounded();
            let session = runtime::props(true, &chain, &signer, Some(account), &network.node_url);
            let handle = runtime::notify_for(Recipient {
                session: tokio::sync::watch::channel(Some(session)).0,
                store: home
                    .join("accounts")
                    .join(account.to_string())
                    .join("store"),
                center: CenterHandle::default(),
                settings: Some(settings()),
                banners: Arc::new(Push::new(
                    Log {
                        network: network.name.clone(),
                        account,
                    },
                    chain.clone(),
                    account,
                    devices.clone(),
                    (apns.clone(), fcm.clone()),
                    http.clone(),
                )),
                said,
            });
            runtime::handle().spawn(badges(heard, network.name.clone(), account));
            carried.insert(account, (signer, handle));
        }
    };
    carry(&mut carried);
    runtime::connected(&client, &status.network, &chain);
    // the roster followed as the app follows it: read again when the height moves
    let mut height = status.height;
    loop {
        std::thread::sleep(STATUS_EVERY);
        if devices.follow(&file) {
            carry(&mut carried);
            tracing::info!(target: "pushd", accounts = carried.len(), "devices read again");
        }
        match runtime::handle().block_on(client.status()) {
            Ok(status) if status.height != height => {
                height = status.height;
                runtime::deployments_checked();
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(target: "pushd", %error, "status not answered");
            }
        }
    }
}

/// One push per credential to a token that is no device's, on each of
/// Apple's servers and on Google's: the answer names the token as bad
/// (`BadDeviceToken`, `UNREGISTERED` or `INVALID_ARGUMENT`) when the
/// credential is good, and the credential (`InvalidProviderToken`, a 401
/// or 403) when it is not. Nothing is printed but the status and the
/// reason.
fn probe(config: &Config) -> Result<(), String> {
    let (apns, fcm) = credentials(config)?;
    let http: Arc<dyn push::Http> = Arc::new(push::Client::new()?);
    let devices = Arc::new(Devices::default());
    let row = |kind: &str, token: &str, env: &str| registry::Row {
        account: 0,
        key: format!("probe-{kind}-{env}"),
        kind: kind.into(),
        token: token.into(),
        env: env.into(),
        at: 1,
    };
    if apns.is_some() {
        // two tokens: one token is one install's, kept on one row
        // in memory: the probe's rows are written nowhere
        let _ = devices.put(row("apns", &"0".repeat(64), "production"));
        let _ = devices.put(row("apns", &"1".repeat(64), "sandbox"));
    }
    if fcm.is_some() {
        let _ = devices.put(row("fcm", "probe-token-that-is-no-device", ""));
    }
    if apns.is_none() && fcm.is_none() {
        return Err("the config names no [apns] or [fcm] credential".into());
    }
    let push = Push::new(
        Log {
            network: "probe".into(),
            account: 0,
        },
        "probe-00000000".into(),
        0,
        devices,
        (apns, fcm),
        http,
    );
    let taken = push.post(
        &Notice {
            title: "pushd probe".into(),
            body: "a push to no device".into(),
            tag: "probe".into(),
            program: "pushd".into(),
            ..Notice::default()
        },
        None,
    );
    println!("probe: the answers are above (a token named as bad = the credential is good)");
    match taken {
        true => Err("a push to a made-up token was taken".into()),
        false => Ok(()),
    }
}

/// The `log` channel: one JSON line per notice the centre lets through.
struct Log {
    network: String,
    account: u64,
}

impl Banners for Log {
    fn post(&self, notice: &Notice, _: Option<Click>) -> bool {
        println!("{}", line(&self.network, self.account, notice));
        true
    }

    fn withdraw(&self, _: &str, tag: &str) {
        let withdrawn = serde_json::json!({
            "channel": "log",
            "network": self.network,
            "account": self.account,
            "withdraw": tag,
        });
        println!("{withdrawn}");
    }
}

/// What an account's centre lets through.
fn settings() -> Settings {
    Settings {
        banners: true,
        in_front: false,
        burst: BURST,
        views: BTreeMap::new(),
        // no one to ask: a program's notices go out until the account's
        // devices say otherwise
        unasked: Some(Permission::Allow),
    }
}

/// One notice as the `log` channel writes it: whose and which, never its
/// words (the journal is not the person's to read).
fn line(network: &str, account: u64, notice: &Notice) -> String {
    serde_json::json!({
        "channel": "log",
        "network": network,
        "account": account,
        "program": notice.program,
        "kind": match notice.kind {
            NoticeKind::Notice => "notice",
            NoticeKind::Call => "call",
        },
        "tag": notice.tag,
        "link": notice.link,
    })
    .to_string()
}

/// An account's badges, per program, each logged when it moves.
async fn badges(
    mut heard: futures::channel::mpsc::UnboundedReceiver<(&'static str, Intent)>,
    network: String,
    account: u64,
) {
    use futures::StreamExt as _;
    let mut kept: BTreeMap<&'static str, i64> = BTreeMap::new();
    while let Some((program, intent)) = heard.next().await {
        let Intent::Badge(count) = intent else {
            continue;
        };
        if kept.insert(program, count) != Some(count) {
            let badge = serde_json::json!({
                "channel": "log",
                "network": network,
                "account": account,
                "program": program,
                "badge": count,
            });
            println!("{badge}");
        }
    }
}

/// The host's services with no screen and no one at it: files under the
/// network's directory, a throwaway key that signs queries, no device key,
/// no banners of its own (each recipient has its channel), no view seated.
struct Headless {
    dirs: Dirs,
    reader: Arc<dyn noded_client::Signer>,
}

impl Headless {
    fn new(home: &Path) -> Result<Self, String> {
        let runtime = home.join("run");
        {
            use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&runtime)
                .and_then(|()| {
                    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))
                })
                .map_err(|error| format!("{}: {error}", runtime.display()))?;
        }
        Ok(Self {
            dirs: Dirs {
                cache: home.join("cache"),
                config: home.join("config"),
                data: home.join("data"),
                remotes: home.join("remotes"),
                runtime,
                views_override: None,
            },
            reader: Arc::new(reader_key()),
        })
    }
}

/// A key made for this process alone, as the desktop's reader.
fn reader_key() -> commonware_cryptography::ed25519::PrivateKey {
    use commonware_codec::DecodeExt as _;
    use rand::RngCore as _;
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    commonware_cryptography::ed25519::PrivateKey::decode(seed.as_slice())
        .expect("32 random bytes decode")
}

/// Banners of the host's own (the app's update notice): pushd raises none.
struct Nowhere;

impl Banners for Nowhere {
    fn post(&self, _: &Notice, _: Option<Click>) -> bool {
        false
    }

    fn withdraw(&self, _: &str, _: &str) {}
}

impl OsServices for Headless {
    fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    fn device_key(&self, _: &str) -> Result<Option<Arc<dyn noded_client::Signer>>, KeyRefusal> {
        Err(KeyRefusal::NoStore("pushd holds no keys".into()))
    }

    fn key_storage(&self, _: &str) -> Result<KeyStorage, KeyRefusal> {
        Err(KeyRefusal::NoStore("pushd holds no keys".into()))
    }

    fn reader(&self) -> Arc<dyn noded_client::Signer> {
        self.reader.clone()
    }

    fn banners(&self) -> &dyn Banners {
        &Nowhere
    }

    fn facts(&self) -> OsFacts {
        OsFacts {
            mac_modifiers: false,
            rss_unit: if cfg!(target_os = "macos") { 1 } else { 1024 },
            touch_first: false,
            device_label: "pushd",
            device_noun: "server",
            one_layout: true,
            floats: false,
            motion: ducktape_host::motion::IOS,
            // its recipients' settings answer for them (`unasked`)
            os_asks_notices: false,
            // notifiers alone, on no screen
            feeds_follow_screen: false,
        }
    }

    fn limits(&self) -> Limits {
        Limits {
            loaders: 1,
            preload: Preload::Headless,
            // the notifiers run in this process: none dies apart from pushd
            snapshot_every: None,
            snapshot_to_disk: false,
        }
    }

    /// pushd ships no view: every notifier is the network's.
    fn bundled_view(&self, _: &abi::BlobId) -> Option<Vec<u8>> {
        None
    }

    fn mint_invite(&self, _: MintInvite) -> Option<Answered> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A notice on the `log` channel is one line of JSON naming the network,
    /// the account, the program and the notice's kind, tag and link, never
    /// its words.
    #[test]
    fn a_log_line_names_whose_notice_it_is_not_its_words() {
        let notice = Notice {
            title: "ada mentioned you".into(),
            body: "look at this".into(),
            tag: "chat/#general".into(),
            program: "chat".into(),
            link: "duck://testkit-0a1b2c3d/chat/general/7".into(),
            ..Notice::default()
        };
        let line = line("testkit", 2, &notice);
        assert!(!line.contains('\n'), "one line: {line}");
        let read: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            read,
            serde_json::json!({
                "channel": "log",
                "network": "testkit",
                "account": 2,
                "program": "chat",
                "kind": "notice",
                "tag": "chat/#general",
                "link": "duck://testkit-0a1b2c3d/chat/general/7",
            })
        );
    }

    /// The config lists networks, the door and the credentials; a network
    /// name is one path segment.
    #[test]
    fn the_config_lists_networks_the_door_and_the_credentials() {
        let config: Config = toml::from_str(
            r#"
            listen = "127.0.0.1:9201"

            [[networks]]
            name = "testkit"
            node_url = "http://127.0.0.1:8844"

            [apns]
            key_file = "/etc/pushd/AuthKey_ABC123.p8"
            key_id = "ABC123"
            team_id = "8464KRUNQG"
            topic = "industries.ducktape.mobile"
            "#,
        )
        .unwrap();
        assert_eq!(config.networks[0].name, "testkit");
        assert_eq!(config.listen.as_deref(), Some("127.0.0.1:9201"));
        let apns = config.apns.unwrap();
        assert_eq!(
            (apns.topic.as_str(), apns.endpoint),
            ("industries.ducktape.mobile", None)
        );
        assert!(config.fcm.is_none());
        assert!(named("testkit") && named("duck-pond_1.x"));
        assert!(!named("..") && !named("a/b") && !named(""));
    }
}
