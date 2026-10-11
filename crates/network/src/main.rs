//! ducktape-network: found or join a Ducktape network on a server. It installs core's published
//! `ducktape` node at the core commit this app pins and modules' published programs at the modules
//! commit it pins (`build.rs` reads both out of Cargo.lock), so the network it founds is one this
//! app speaks. Everything lives under one directory (`--dir`): the node binary, the programs, the
//! founding file and the node's workspace; the node runs as a systemd service.
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod address;
#[cfg(test)] // build.rs reads the pins; the tests here are its
mod pins;
mod view_pack;

use address::{Address, Transport};

/// The core commit the app pins: the node it speaks, published as the release `bin-<commit>`.
const CORE: &str = env!("DUCKTAPE_CORE");
/// The modules commit the app pins: the programs and views, published as `wasm-<commit>`.
const MODULES: &str = env!("DUCKTAPE_MODULES");
/// This binary's target triple, which names core's published node archive.
const TARGET: &str = env!("DUCKTAPE_TARGET");
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (core ",
    env!("DUCKTAPE_CORE"),
    ", modules ",
    env!("DUCKTAPE_MODULES"),
    ")"
);
const HTTP_PORT: u16 = 8844;
/// ponytail: one network per user (the unit's name is fixed); name the unit after the network
/// when one server runs two.
const UNIT: &str = "ducktape-network";

#[derive(Parser)]
#[command(version = VERSION, about, long_about = None, arg_required_else_help = true)]
struct Cli {
    /// Where the network lives: the node, the programs, the founding file and the node's workspace
    #[arg(long, global = true, default_value_os_t = default_dir())]
    dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Found a network with this server as its one validator, and run its node as a service
    Found {
        #[arg(
            help = "the network's name, which every frame and invite on it carries \
                      (letters, digits, `-`, `_`, `.`)"
        )]
        name: String,
        #[command(flatten)]
        reach: Reach,
        #[command(flatten)]
        node: LocalNode,
        #[arg(
            long,
            help = "a directory of unpacked modules wasm with its SHA256SUMS, instead of \
                    downloading the release wasm-<modules commit>"
        )]
        wasm: Option<PathBuf>,
    },
    #[command(
        about = "Join a network another server founded: enroll, wait for the seat, sync, run as a service"
    )]
    Join {
        #[arg(help = "the founder's node, as `found` printed it")]
        source: String,
        #[command(flatten)]
        reach: Reach,
        #[arg(long, help = "the founder's invite; none when the door is open")]
        invite: Option<String>,
        #[command(flatten)]
        node: LocalNode,
    },
    /// The node's height, tip, root, epoch and identity
    Status,
    /// The installed `ducktape` CLI on this network's workspace:
    /// `cli invite`, `cli vote promote KEY`, `cli logs`, `cli wallet new NAME`, …
    #[command(alias = "ducktape")]
    Cli {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Args)]
struct Reach {
    #[arg(
        long,
        help = "where peers dial this node: tcp://host:port, tls://host:port, ws://host[:port] \
                or wss://host[:port]; the node listens on every interface"
    )]
    address: Address,
    #[arg(long, default_value_t = HTTP_PORT, help = "the port of the node's HTTP API, which the app and joining servers reach")]
    http_port: u16,
}

#[derive(Args)]
struct LocalNode {
    #[arg(
        long,
        help = "a local `ducktape` binary, instead of downloading the release bin-<core commit>"
    )]
    ducktape: Option<PathBuf>,
}

fn default_dir() -> PathBuf {
    PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".ducktape-network")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // absolute: the service unit names these paths
    let dir = std::path::absolute(&cli.dir)?;
    match cli.cmd {
        Cmd::Found {
            name,
            reach,
            node,
            wasm,
        } => found(
            &dir,
            &name,
            &reach,
            node.ducktape.as_deref(),
            wasm.as_deref(),
        ),
        Cmd::Join {
            source,
            reach,
            invite,
            node,
        } => join(
            &dir,
            &source,
            &reach,
            invite.as_deref(),
            node.ducktape.as_deref(),
        ),
        Cmd::Status => run(ducktape(&dir, &installed(&dir)?).arg("status")),
        Cmd::Cli { args } => {
            Err(ducktape(&dir, &installed(&dir)?).args(args).exec()).context("ducktape")
        }
    }
}

// ---- found and join ----

fn found(
    dir: &Path,
    name: &str,
    reach: &Reach,
    local_node: Option<&Path>,
    local_wasm: Option<&Path>,
) -> Result<()> {
    let nameable = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    if name.is_empty() || !name.chars().all(nameable) {
        bail!("a network's name is letters, digits, `-`, `_` and `.`: {name:?}");
    }
    fresh(dir)?;
    let ducktape_bin = install_node(dir, local_node)?;
    let wasm = match local_wasm {
        Some(wasm) => wasm.to_path_buf(),
        None => published_wasm(dir)?,
    };
    verify(&wasm)?;
    let programs = dir.join("programs");
    pack_programs(&wasm, &programs)?;
    fs::write(programs.join("forge.params"), abi::encode(&forge_bounds()))?;
    let key = out(ducktape(dir, &ducktape_bin).arg("identity"))?;
    let path = dir.join("founding.toml");
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    fs::write(&path, founding(name, now, key.trim(), &reach.address))?;
    run(ducktape(dir, &ducktape_bin).arg("init").arg(&path))?;
    let node = Node::reachable(&reach.address, inner()?, reach.http_port);
    let source = reach.address.source(reach.http_port);
    let cli = match dir == default_dir() {
        true => "ducktape-network".to_string(),
        false => format!("ducktape-network --dir {}", dir.display()),
    };
    if !start(dir, &ducktape_bin, &node)? {
        println!("once it runs, the app connects to {source}");
        println!("and `{cli} cli invite` mints an invite for another server");
        return Ok(());
    }
    println!("the app connects to {source}");
    let invite = out(ducktape(dir, &ducktape_bin).arg("invite"))?;
    println!("invite: one enrollment, valid 72 hours (`{cli} cli invite` mints more)");
    println!("on another server, with this tool:");
    println!(
        "  ducktape-network join {source} --address {} --invite {}",
        reach.address.peer_form(),
        invite.trim()
    );
    Ok(())
}

fn join(
    dir: &Path,
    source: &str,
    reach: &Reach,
    invite: Option<&str>,
    local_node: Option<&Path>,
) -> Result<()> {
    fresh(dir)?;
    let ducktape_bin = install_node(dir, local_node)?;
    let mut enroll = ducktape(dir, &ducktape_bin);
    enroll
        .args(["join", source, "--address"])
        .arg(reach.address.to_string());
    if let Some(invite) = invite {
        enroll.args(["--invite", invite]);
    }
    println!("enrolling through {source}, then waiting for the epoch that seats this node");
    run(&mut enroll)?;
    let node = Node::reachable(&reach.address, inner()?, reach.http_port);
    start(dir, &ducktape_bin, &node)?;
    let key = out(ducktape(dir, &ducktape_bin).arg("identity"))?;
    println!("joined as a resident: it follows the chain and does not vote yet");
    println!("the founder seats it as a validator, on the founder's server:");
    println!("  ducktape-network cli vote promote {}", key.trim());
    Ok(())
}

/// A directory that holds a network already is never founded or joined over.
fn fresh(dir: &Path) -> Result<()> {
    if dir.join("home/network").exists() {
        bail!(
            "{} holds a network already: `ducktape-network --dir {0} status` shows it",
            dir.display()
        );
    }
    Ok(fs::create_dir_all(dir.join("home"))?)
}

/// The founding file core's `ducktape init` reads (its README, "Found a network"): this node the
/// one validator, the boot set and the apps, and Explorer as a view with no program behind it.
/// Paths are relative to the file, which sits beside `programs/`.
fn founding(name: &str, time: u128, key: &str, address: &Address) -> String {
    format!(
        r#"network = "{name}"
time = {time}
epoch_length = 64
block_time_ms = 1000

[roles]
registry = "module-registry"
validators = "valset"
identity = "identity"

[[validators]]
key = "{key}"
address = "{address}"

[[programs]]
id = "module-registry"
code = "programs/module_registry.wasm"

[[programs]]
id = "valset"
code = "programs/valset.wasm"

[[programs]]
id = "identity"
code = "programs/identity.wasm"

[[programs]]
id = "chat"
code = "programs/chat.wasm"

[[programs]]
id = "forge"
code = "programs/forge.wasm"
params = "programs/forge.params"

# A view with no program behind it: listed by the registry, never admitted.
[[views]]
name = "explorer"
code = "programs/explorer_view.wasm"
"#
    )
}

/// The bounds forge's `init` decodes from its founding params, as qa's kit founds it.
/// ponytail: `forge::Bounds::default()` (modules#303) once the app's modules pin has it.
fn forge_bounds() -> forge::Bounds {
    forge::Bounds {
        max_objects: 1_000_000,
        max_delta_depth: 64,
        max_object_size: 256 << 20,
        push_walk: 10_000,
        fetch_walk: 1_000_000,
        merge_cost: 1024,
        page_size: 128,
        log_walk: 10_000,
        tree_walk: 1024,
        diff_bytes: 32 << 20,
        blob_bytes: 256 << 10,
        record_bytes: 64 << 10,
    }
}

// ---- the node and the programs ----

/// `ducktape` on this network's workspace.
fn ducktape(dir: &Path, bin: &Path) -> Command {
    let mut c = Command::new(bin);
    c.arg("--workspace").arg(dir.join("home"));
    c
}

fn installed(dir: &Path) -> Result<PathBuf> {
    let bin = dir.join("bin/ducktape");
    if !bin.exists() {
        bail!(
            "no network in {}: `found` or `join` one first",
            dir.display()
        );
    }
    Ok(bin)
}

/// `<dir>/bin/ducktape`: the local binary, else core's published node at the pinned commit.
/// Core publishes no checksum for it: the trust is GitHub's TLS and a release tag that names the
/// commit.
fn install_node(dir: &Path, local: Option<&Path>) -> Result<PathBuf> {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin)?;
    let ducktape = bin.join("ducktape");
    let _ = fs::remove_file(&ducktape);
    match local {
        Some(local) => {
            fs::copy(local, &ducktape)
                .with_context(|| format!("--ducktape {}", local.display()))?;
        }
        None => {
            let asset = format!("ducktape-{TARGET}.tar.gz");
            let archive = bin.join(&asset);
            download("ducktape", CORE, &format!("bin-{CORE}"), &asset, &archive)?;
            run(Command::new("tar")
                .arg("-xzf")
                .arg(&archive)
                .arg("-C")
                .arg(&bin)
                .arg("ducktape"))?;
            fs::remove_file(&archive)?;
        }
    }
    println!(
        "node: {}",
        out(Command::new(&ducktape).arg("version"))?.trim()
    );
    Ok(ducktape)
}

/// modules' published wasm at the pinned commit, unpacked into `<dir>/wasm` (verified by the
/// caller, as a local directory is).
fn published_wasm(dir: &Path) -> Result<PathBuf> {
    let wasm = dir.join("wasm");
    let _ = fs::remove_dir_all(&wasm);
    fs::create_dir_all(&wasm)?;
    let archive = wasm.join("modules-wasm.tar.gz");
    let release = format!("wasm-{MODULES}");
    download(
        "modules",
        MODULES,
        &release,
        "modules-wasm.tar.gz",
        &archive,
    )?;
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&wasm))?;
    fs::remove_file(&archive)?;
    Ok(wasm)
}

fn download(repo: &str, commit: &str, release: &str, asset: &str, to: &Path) -> Result<()> {
    let url = format!(
        "https://github.com/ducktape-industries/{repo}/releases/download/{release}/{asset}"
    );
    println!("downloading {url}");
    run(Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--retry",
            "3",
            "--output",
        ])
        .arg(to)
        .arg(&url))
    .with_context(|| {
        format!(
            "ducktape-industries/{repo} has no release {release} with {asset}: \
             commit {commit} is not published (pass a local build instead: see --help)"
        )
    })
}

const ARTIFACTS: [&str; 16] = [
    "module_registry",
    "valset",
    "identity",
    "chat",
    "forge",
    "module_registry.describe",
    "valset.describe",
    "identity.describe",
    "chat.describe",
    "forge.describe",
    "settings_view",
    "node_view",
    "members_view",
    "chat_view",
    "forge_view",
    "explorer_view",
];

/// Every program, describe module and view is listed in `SHA256SUMS` and matches its line, as
/// qa's kit checks it.
fn verify(wasm: &Path) -> Result<()> {
    let sums = fs::read_to_string(wasm.join("SHA256SUMS"))
        .with_context(|| format!("no SHA256SUMS in {}", wasm.display()))?;
    for name in ARTIFACTS {
        let file = format!("{name}.wasm");
        let listed = sums
            .lines()
            .find_map(|line| {
                let (digest, name) = line.split_once("  ")?;
                (name == file).then_some(digest)
            })
            .with_context(|| format!("SHA256SUMS lists no {file}"))?;
        let bytes = fs::read(wasm.join(&file)).with_context(|| format!("no {file}"))?;
        if hex(&Sha256::digest(&bytes)) != listed {
            bail!("{file} does not match its SHA256SUMS line");
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Each view and each describe module into its program, as qa's kit packs them (modules leaves
/// packing to whoever founds).
fn pack_programs(wasm: &Path, programs: &Path) -> Result<()> {
    fs::create_dir_all(programs)?;
    for name in [
        "module_registry",
        "valset",
        "identity",
        "chat",
        "forge",
        "explorer_view",
    ] {
        fs::copy(
            wasm.join(format!("{name}.wasm")),
            programs.join(format!("{name}.wasm")),
        )?;
    }
    for (program, view) in [
        ("module_registry", "settings_view"),
        ("valset", "node_view"),
        ("identity", "members_view"),
        ("chat", "chat_view"),
        ("forge", "forge_view"),
    ] {
        let path = programs.join(format!("{program}.wasm"));
        let packed = view_pack::embed(
            &fs::read(&path)?,
            &fs::read(wasm.join(format!("{view}.wasm")))?,
        )
        .map_err(|e| anyhow::anyhow!("{view}: {e}"))?;
        fs::write(&path, packed)?;
    }
    for program in ["module_registry", "valset", "identity", "chat", "forge"] {
        let path = programs.join(format!("{program}.wasm"));
        let module = wasm.join(format!("{program}.describe.wasm"));
        let packed = view_pack::embed_describe(&fs::read(&path)?, &fs::read(&module)?)
            .map_err(|e| anyhow::anyhow!("{}: {e}", module.display()))?;
        fs::write(&path, packed)?;
    }
    Ok(())
}

// ---- the service ----

struct Node {
    listen: SocketAddr,
    http: SocketAddr,
    tls: Option<SocketAddr>,
}

impl Node {
    /// The sockets a node peers reach at `address` listens on, as qa's kit runs one: every
    /// interface for the transport's port and the HTTP API, and `inner` (loopback) for the
    /// `--listen` socket a transport other than tcp leaves unused.
    fn reachable(address: &Address, inner: SocketAddr, http_port: u16) -> Node {
        let every = address.every_interface();
        let http = SocketAddr::new(every, http_port);
        match address.transport {
            Transport::Tcp => Node {
                listen: SocketAddr::new(every, address.port),
                http,
                tls: None,
            },
            Transport::Tls => Node {
                listen: inner,
                http,
                tls: Some(SocketAddr::new(every, address.port)),
            },
            Transport::Ws | Transport::Wss => Node {
                listen: inner,
                http,
                tls: None,
            },
        }
    }

    fn args(&self) -> Vec<String> {
        let mut args = vec![
            "run".to_string(),
            "--listen".to_string(),
            self.listen.to_string(),
            "--http".to_string(),
            self.http.to_string(),
        ];
        if let Some(tls) = self.tls {
            args.extend(["--tls".to_string(), tls.to_string()]);
        }
        args
    }
}

fn inner() -> Result<SocketAddr> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?.local_addr()?)
}

/// Runs the node as a systemd service and waits for it to answer; on macOS prints the command
/// instead (no launchd yet) and answers false.
fn start(dir: &Path, ducktape: &Path, node: &Node) -> Result<bool> {
    let mut exec = vec![
        ducktape.display().to_string(),
        "--workspace".to_string(),
        dir.join("home").display().to_string(),
    ];
    exec.extend(node.args());
    if cfg!(target_os = "macos") {
        println!("no service on macOS yet: run the node with");
        println!("  {}", exec.join(" "));
        return Ok(false);
    }
    let root = unsafe { libc::geteuid() } == 0;
    let (units, wanted_by, systemctl): (PathBuf, _, &[&str]) = match root {
        true => (
            PathBuf::from("/etc/systemd/system"),
            "multi-user.target",
            &["systemctl"],
        ),
        false => (
            env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".config")
                })
                .join("systemd/user"),
            "default.target",
            &["systemctl", "--user"],
        ),
    };
    fs::create_dir_all(&units)?;
    let path = units.join(format!("{UNIT}.service"));
    fs::write(&path, unit(&exec, wanted_by))?;
    println!("service: {}", path.display());
    let ctl = |args: &[&str]| {
        run(Command::new(systemctl[0]).args(&systemctl[1..]).args(args)).with_context(|| {
            format!(
                "systemd did not take the unit; run the node by hand:\n  {}",
                exec.join(" ")
            )
        })
    };
    ctl(&["daemon-reload"])?;
    ctl(&["enable", UNIT])?;
    ctl(&["restart", UNIT])?;
    if !root {
        println!(
            "a user service stops when you log out: `loginctl enable-linger` keeps it running"
        );
    }
    for _ in 0..60 {
        let up = self::ducktape(dir, ducktape)
            .arg("status")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success();
        if up {
            run(self::ducktape(dir, ducktape).arg("status"))?;
            return Ok(true);
        }
        thread::sleep(Duration::from_secs(1));
    }
    bail!(
        "the node does not answer after a minute: `journalctl {}-u {UNIT}` shows why",
        if root { "" } else { "--user " }
    )
}

fn unit(exec: &[String], wanted_by: &str) -> String {
    // systemd's own quoting: `%` and `$` would expand, `"` and `\` would end the word
    let quoted = |arg: &String| {
        format!(
            "\"{}\"",
            arg.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%")
                .replace('$', "$$")
        )
    };
    let exec = exec.iter().map(quoted).collect::<Vec<_>>().join(" ");
    format!(
        "[Unit]
Description=Ducktape node (ducktape-network)

[Service]
ExecStart={exec}
Restart=on-failure
LimitNOFILE=65536

[Install]
WantedBy={wanted_by}
"
    )
}

// ---- processes ----

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("{cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?}: {status}");
    }
    Ok(())
}

fn out(cmd: &mut Command) -> Result<String> {
    let output = cmd
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("{cmd:?}"))?;
    if !output.status.success() {
        bail!("{cmd:?}: {}", output.status);
    }
    Ok(String::from_utf8(output.stdout)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmparser::{Parser, Payload};

    #[test]
    fn the_founding_names_this_node_the_programs_and_explorer() {
        let address: Address = "tcp://203.0.113.7:9000".parse().unwrap();
        let text = founding("mynet", 1_700_000_000_000, "ab12", &address);
        let expected = [
            "network = \"mynet\"\n",
            "time = 1700000000000\n",
            "epoch_length = 64\n",
            "block_time_ms = 1000\n",
            "[roles]\nregistry = \"module-registry\"\nvalidators = \"valset\"\nidentity = \"identity\"\n",
            "[[validators]]\nkey = \"ab12\"\naddress = \"tcp://203.0.113.7:9000\"\n",
            "id = \"forge\"\ncode = \"programs/forge.wasm\"\nparams = \"programs/forge.params\"\n",
            "[[views]]\nname = \"explorer\"\ncode = \"programs/explorer_view.wasm\"\n",
        ];
        for line in expected {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
        let programs: Vec<&str> = text
            .lines()
            .filter_map(|line| line.strip_prefix("id = "))
            .collect();
        assert_eq!(
            programs,
            [
                "\"module-registry\"",
                "\"valset\"",
                "\"identity\"",
                "\"chat\"",
                "\"forge\""
            ]
        );
        assert!(!text.contains("member_cap") && !text.contains("ping"));
    }

    fn sums(dir: &Path) -> String {
        ARTIFACTS
            .iter()
            .map(|name| {
                let bytes = fs::read(dir.join(format!("{name}.wasm"))).unwrap();
                format!("{}  {name}.wasm\n", hex(&Sha256::digest(&bytes)))
            })
            .collect()
    }

    /// Each artifact a distinct valid module: `(module (func))` with `i` functions.
    fn artifacts() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (i, name) in ARTIFACTS.iter().enumerate() {
            let module = wat::parse_str(format!("(module {})", "(func)".repeat(i))).unwrap();
            fs::write(dir.path().join(format!("{name}.wasm")), module).unwrap();
        }
        fs::write(dir.path().join("SHA256SUMS"), sums(dir.path())).unwrap();
        dir
    }

    #[test]
    fn wasm_is_taken_only_when_every_artifact_matches_its_sum() {
        let dir = artifacts();
        verify(dir.path()).unwrap();

        let chat = dir.path().join("chat.wasm");
        let mut bytes = fs::read(&chat).unwrap();
        bytes.push(0);
        fs::write(&chat, bytes).unwrap();
        let tampered = verify(dir.path()).unwrap_err().to_string();
        assert_eq!(tampered, "chat.wasm does not match its SHA256SUMS line");

        let fresh = artifacts();
        let listed = sums(fresh.path());
        let unlisted: String = listed
            .lines()
            .filter(|line| !line.ends_with("  explorer_view.wasm"))
            .map(|line| format!("{line}\n"))
            .collect();
        fs::write(fresh.path().join("SHA256SUMS"), unlisted).unwrap();
        let missing = verify(fresh.path()).unwrap_err().to_string();
        assert_eq!(missing, "SHA256SUMS lists no explorer_view.wasm");
    }

    fn custom(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
        Parser::new(0)
            .parse_all(bytes)
            .find_map(|payload| match payload.unwrap() {
                Payload::CustomSection(section) if section.name() == name => {
                    Some(section.data().to_vec())
                }
                _ => None,
            })
    }

    #[test]
    fn each_program_carries_its_view_and_describe_module() {
        let wasm = artifacts();
        let programs = tempfile::tempdir().unwrap();
        pack_programs(wasm.path(), programs.path()).unwrap();
        let read = |dir: &Path, name: &str| fs::read(dir.join(format!("{name}.wasm"))).unwrap();
        for (program, view) in [
            ("module_registry", "settings_view"),
            ("valset", "node_view"),
            ("identity", "members_view"),
            ("chat", "chat_view"),
            ("forge", "forge_view"),
        ] {
            let packed = read(programs.path(), program);
            assert_eq!(
                custom(&packed, view_pack::VIEW_SECTION),
                Some(read(wasm.path(), view)),
                "{program}"
            );
            assert_eq!(
                custom(&packed, view_pack::DESCRIBE_SECTION),
                Some(read(wasm.path(), &format!("{program}.describe"))),
                "{program}"
            );
        }
        assert_eq!(
            read(programs.path(), "explorer_view"),
            read(wasm.path(), "explorer_view")
        );
    }

    #[test]
    fn a_reachable_node_listens_on_every_interface() {
        let inner: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let node = |text: &str| Node::reachable(&text.parse().unwrap(), inner, 8844).args();
        assert_eq!(
            node("tcp://203.0.113.7:9000"),
            ["run", "--listen", "0.0.0.0:9000", "--http", "0.0.0.0:8844"]
        );
        assert_eq!(
            node("tls://a.example.org:9443"),
            [
                "run",
                "--listen",
                "127.0.0.1:40000",
                "--http",
                "0.0.0.0:8844",
                "--tls",
                "0.0.0.0:9443"
            ]
        );
        assert_eq!(
            node("wss://a.example.org"),
            [
                "run",
                "--listen",
                "127.0.0.1:40000",
                "--http",
                "0.0.0.0:8844"
            ]
        );
    }

    #[test]
    fn the_unit_quotes_what_systemd_would_expand() {
        let exec = ["/srv/a b/ducktape".to_string(), "100%$x".to_string()];
        assert!(
            unit(&exec, "default.target")
                .contains("ExecStart=\"/srv/a b/ducktape\" \"100%%$$x\"\n")
        );
    }
}
