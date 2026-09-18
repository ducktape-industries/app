use super::*;

#[test]
fn a_chat_load_answers_for_the_huddle_only_when_it_loaded_the_huddles_channel() {
    let member = |is_you: bool| HuddleParticipant {
        key: "aa".into(),
        label: "aa".into(),
        initials: "A".into(),
        is_agent: false,
        is_you,
        joined_at: 0,
        node: "aa11".into(),
    };
    let idle = HuddleAfterLoad::default();

    // Not in a huddle: the loaded channel's roster is the whole answer.
    let joined = huddle_after_load(
        true,
        idle.joined,
        idle.channel.clone(),
        idle.channel_name.clone(),
        idle.roster.clone(),
        "eng".into(),
        "Engineering".into(),
        vec![member(true)],
    );
    assert!(joined.joined);
    assert_eq!(joined.channel, "eng");
    assert_eq!(joined.channel_name, "Engineering");
    assert_eq!(joined.roster.len(), 1);

    // NOW CLICK ANOTHER ROOM. Its roster is a different conversation's, and
    // reading the huddle off it used to cut the call's media (the session is
    // subscribed on `joined`) and blank the channel `leave_huddle_here` needs.
    let switched = huddle_after_load(
        true,
        joined.joined,
        joined.channel.clone(),
        joined.channel_name.clone(),
        joined.roster.clone(),
        "general".into(),
        "General".into(),
        Vec::new(),
    );
    assert_eq!(switched, joined, "another room's load is not the huddle's");

    // Back on the huddle's own channel, a roster without her ends it.
    let left = huddle_after_load(
        true,
        joined.joined,
        joined.channel.clone(),
        joined.channel_name.clone(),
        joined.roster.clone(),
        "eng".into(),
        "Engineering".into(),
        vec![member(false)],
    );
    assert_eq!(left, idle);

    // And a resync that carried no chat at all says nothing either way.
    let quiet = huddle_after_load(
        false,
        joined.joined,
        joined.channel.clone(),
        joined.channel_name.clone(),
        joined.roster.clone(),
        "eng".into(),
        "Engineering".into(),
        Vec::new(),
    );
    assert_eq!(quiet, joined);
}

/// THE ROSTER IS NOT THE HOST'S. Who a network calls a validator, who it
/// calls a resident, and whether THIS node may act on either are folds of
/// `valset` — and every screen that draws one reads it for itself through
/// the kernel. A host-side row type is how those folds diverge: the app
/// would decide the gate while the view decided the word, and a view swap
/// could no longer change either. The directory beneath is walked rather
/// than a fixed file list, so a new backend file cannot reintroduce one.
#[test]
fn the_backend_builds_no_roster_of_its_own() {
    let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/backend");
    let mut offenders = Vec::new();
    let mut pending = vec![backend.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("the backend directory is readable") {
            let path = entry.expect("a backend entry").path();
            if path.is_dir() {
                // this walk names the banned symbols, and it lives here
                let is_the_suite = path.file_name().is_some_and(|name| name == "tests");
                if !is_the_suite {
                    pending.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|kind| kind != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("a backend file is readable");
            let named = [
                "MemberRow",
                "MembersData",
                "load_members",
                "members_is_admin",
            ]
            .into_iter()
            .filter(|name| source.contains(name))
            .collect::<Vec<_>>();
            if !named.is_empty() {
                offenders.push(format!("{}: {named:?}", path.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the roster belongs to the members view: {offenders:?}"
    );
}

/// The shell owns ONE transient layer now — the bell — and Escape names it
/// only when it is up. Every other layer is a view's own: the palette claims
/// its chord and answers its own Escape, like the chat menus and the pages
/// cards before it.
#[test]
fn escape_ladder_names_the_topmost_transient_layer_only() {
    assert_eq!(escape_target("x".into(), true), "");
    assert_eq!(escape_target("escape".into(), true), "bell");
    assert_eq!(escape_target("escape".into(), false), "");
}

#[test]
fn the_two_ladder_readers_enumerate_the_same_layers() {
    assert_eq!(topmost_overlay(true), "bell");
    assert_eq!(topmost_overlay(false), "");
}

#[test]
fn files_base64_round_trips() {
    for sample in [
        b"".as_slice(),
        b"a".as_slice(),
        b"ab".as_slice(),
        b"abc".as_slice(),
        b"hello duckfs \xf0\x9f\xa6\x86".as_slice(),
    ] {
        let encoded = base64_encode(sample);
        assert_eq!(
            base64_decode(&encoded).as_deref(),
            Some(sample),
            "{encoded}"
        );
    }
    assert_eq!(base64_encode(b"abc"), "YWJj");
    assert_eq!(base64_encode(b"ab"), "YWI=");
    // MALFORMED IS A REFUSAL, NOT EMPTY BYTES. The handwritten decoder this
    // replaced stopped at padding per quartet (`Zg==Zg==` read as `ff`) and
    // read a lone `Z` as nothing; a read page that decodes to `None` fails
    // the read upstream instead of showing an empty file.
    for malformed in [
        "Zg==Zg==", "Z", "Zg", "Zg=", "Zg===", "Zh==", "Y*Jj", "YWJj\n",
    ] {
        assert_eq!(base64_decode(malformed), None, "{malformed}");
    }
}

/// THE TAB-SWITCH GATE. Four planes used to refetch on every tab move —
/// members, governance, agents, account — regardless of the destination, so a
/// click into Files paid four `/v1/query` round trips for rows nothing on
/// screen reads.
#[test]
fn a_tab_move_only_refetches_what_its_destination_draws() {
    let tabs = [
        ShellTab::View("chat"),
        ShellTab::View("pages"),
        ShellTab::View("forge"),
        ShellTab::View("agents"),
        ShellTab::View("files"),
        ShellTab::View("explorer"),
        ShellTab::View("node"),
        ShellTab::View("members"),
        ShellTab::View("governance"),
        ShellTab::View("settings"),
    ];

    // every plane left is one pane's: Settings draws the account card, Forge
    // the org "about", and proposals and agent rows belong to one pane each.
    // The ROSTER is on no line here at all — the members, governance, node,
    // forge and settings views each read the valset for themselves, so no tab
    // click loads one.
    for (plane, drawn) in [
        ("members", &[][..]),
        ("governance", &[ShellTab::View("governance")][..]),
        ("agents", &[ShellTab::View("agents")][..]),
        (
            "account",
            &[ShellTab::View("forge"), ShellTab::View("settings")][..],
        ),
        // an unknown plane name is nobody's — a typo must not silently reopen
        // the storm by answering true.
        ("explorer", &[][..]),
    ] {
        let readers: Vec<ShellTab> = tabs
            .iter()
            .copied()
            .filter(|tab| tab_reads_plane(*tab, plane.into()))
            .collect();
        assert_eq!(readers, drawn, "exactly these tabs draw {plane}");
    }
}

/// The launch window reads the workspace files through the crate that wrote
/// them, never a line parser: the chain id keeps its `#hex` half, a
/// two-validator descriptor (the multi-line array `node admit` writes) still
/// yields the founding key, and a wildcard `http_listen` dials loopback.
#[test]
fn workspace_facts_come_from_the_crate_that_wrote_them() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("mynet-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let founder = "aa".repeat(32);
    let admitted = "bb".repeat(32);
    workspace_config::NetworkDescriptor {
        chain_id: "mynet#a1b2c3d4".into(),
        validators: vec![founder.clone(), admitted],
        bootstrap: vec![],
        reach: vec![],
        coordination: None,
        block_time_ms: workspace_config::DEFAULT_BLOCK_TIME_MS,
        modules: vec![],
        genesis: String::new(),
    }
    .save(&dir.join("network.toml"))
    .unwrap();
    let descriptor = std::fs::read_to_string(dir.join("network.toml")).unwrap();
    assert!(
        descriptor.contains("validators = [\n"),
        "two validators serialize as a multi-line array:\n{descriptor}"
    );
    std::fs::write(
        dir.join("node.toml"),
        r#"network = "network.toml"
key_file = "node.key"
listen = "0.0.0.0:52200"
advertised = "overlay"
storage_dir = "data"
http_listen = "0.0.0.0:8844"
gateway_listen = "127.0.0.1:0"
rpc_listen = "127.0.0.1:8845"
wireguard_listen = "0.0.0.0:51820"
invite_listen = "0.0.0.0:51821"
wireguard_advertised = "auto"
primary_coordinator = "none"
coordinator_relay = "none"
checkpoint_blocks = 32
"#,
    )
    .unwrap();

    assert_eq!(
        workspaces_in(root.path()),
        vec![("mynet#a1b2c3d4".to_string(), dir.clone())]
    );
    assert_eq!(workspace_identity(&dir), Some(short_label(&founder)));
    assert_eq!(
        workspace_endpoint_in(&serde_json::json!({}), "mynet#a1b2c3d4", &dir).as_deref(),
        Some("http://127.0.0.1:8844")
    );
}

/// TWO WORKSPACES, ONE PORT (#15). `node init` twice on a device registers
/// both on the default endpoint, so the endpoint names neither. Opening a row
/// records its chain id, and from then on that endpoint resolves to that
/// chain's workspace — its keystore root and data dir — never the sibling
/// registered first.
#[test]
fn a_picked_chain_opens_its_own_workspace_on_a_shared_endpoint() {
    let root = tempfile::tempdir().unwrap();
    let register = |dir_name: &str, chain_id: &str| {
        let dir = root.path().join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        workspace_config::NetworkDescriptor {
            chain_id: chain_id.into(),
            validators: vec!["aa".repeat(32)],
            bootstrap: vec![],
            reach: vec![],
            coordination: None,
            block_time_ms: workspace_config::DEFAULT_BLOCK_TIME_MS,
            modules: vec![],
            genesis: String::new(),
        }
        .save(&dir.join("network.toml"))
        .unwrap();
        std::fs::write(
            dir.join("node.toml"),
            r#"network = "network.toml"
key_file = "node.key"
listen = "0.0.0.0:52200"
advertised = "overlay"
storage_dir = "data"
http_listen = "0.0.0.0:18519"
gateway_listen = "127.0.0.1:0"
rpc_listen = "127.0.0.1:18520"
wireguard_listen = "0.0.0.0:51820"
invite_listen = "0.0.0.0:51821"
wireguard_advertised = "auto"
primary_coordinator = "none"
coordinator_relay = "none"
checkpoint_blocks = 32
"#,
        )
        .unwrap();
        dir
    };
    let dead = register("walk", "walk#37589218");
    let healthy = register("walk-2", "walk#0e1b62f1");
    let no_prefs = serde_json::json!({});
    let endpoint = workspace_endpoint_in(&no_prefs, "walk#0e1b62f1", &healthy).unwrap();
    assert_eq!(
        workspace_endpoint_in(&no_prefs, "walk#37589218", &dead).as_deref(),
        Some(endpoint.as_str())
    );

    for (chain_id, dir) in [
        ("walk#0e1b62f1", &healthy),
        ("walk#37589218", &dead),
        ("walk#0e1b62f1", &healthy),
    ] {
        note_served_chain(&endpoint, chain_id);
        assert_eq!(
            workspace_serving(&no_prefs, root.path(), &endpoint),
            Some((chain_id.to_string(), dir.clone())),
            "opening {chain_id} resolves its own workspace"
        );
    }
    // a chain no workspace on this endpoint holds is a remote, not a sibling.
    note_served_chain(&endpoint, "team#c0ffee");
    assert_eq!(workspace_serving(&no_prefs, root.path(), &endpoint), None);
}

/// A WORKSPACE'S NODE RPC URL (#92). Settings can point a workspace at its node
/// on another machine; the URL is kept per chain id beside the last-used stamp,
/// read back through the prefs file's own encoding, and it — not `node.toml` —
/// is what the workspace dials while it is set. Cleared, or not a URL (a hand
/// edit), the workspace dials `node.toml` again.
#[test]
fn a_workspace_dials_its_stored_node_url_else_its_node_toml() {
    let (home, _) = joined_workspace("dognet#920");
    let dir = home.path().join("dognet#920");
    let local = Some("http://127.0.0.1:18619");
    let remote = "http://100.92.85.92:28990";
    let dials = |prefs: &serde_json::Value| workspace_endpoint_in(prefs, "dognet#920", &dir);
    assert_eq!(dials(&serde_json::json!({})).as_deref(), local);

    let mut prefs = serde_json::json!({"networks": {"dognet#920": {"last_used": 7}}});
    store_endpoint_override(&mut prefs, "dognet#920", remote);
    let written = serde_json::to_vec_pretty(&prefs).unwrap();
    let mut prefs: serde_json::Value = serde_json::from_slice(&written).unwrap();
    assert_eq!(
        prefs["networks"]["dognet#920"],
        serde_json::json!({"last_used": 7, "endpoint": remote})
    );
    assert_eq!(dials(&prefs).as_deref(), Some(remote));
    assert_eq!(
        endpoint_override(&prefs, "dognet#920").as_deref(),
        Some(remote)
    );
    assert_eq!(
        workspace_endpoint_in(&prefs, "catnet#920", &dir).as_deref(),
        local,
        "another chain's URL is not this one's"
    );

    store_endpoint_override(&mut prefs, "dognet#920", "");
    assert_eq!(
        prefs["networks"]["dognet#920"],
        serde_json::json!({"last_used": 7}),
        "clearing keeps the last-used stamp"
    );
    assert_eq!(dials(&prefs).as_deref(), local);
    assert_eq!(endpoint_override(&prefs, "dognet#920"), None);

    for invalid in [
        serde_json::json!("ftp://100.92.85.92:28990"),
        serde_json::json!("http://100.92.85.92:28990/v1"),
        serde_json::json!("100.92.85.92:28990"),
        serde_json::json!(""),
        serde_json::json!(28990),
    ] {
        prefs["networks"]["dognet#920"]["endpoint"] = invalid.clone();
        assert_eq!(dials(&prefs).as_deref(), local, "{invalid} is not a URL");
        assert_eq!(endpoint_override(&prefs, "dognet#920"), None);
    }
}

/// WHAT A NODE RPC URL IS (#92): `http` or `https`, a host (loopback too), an
/// optional port and nothing else — in the origin form the rpc client keys its
/// connections by, so a stored URL and the session dialled at it are one name.
#[test]
fn a_node_rpc_url_is_an_http_origin_and_nothing_else() {
    for (typed, origin) in [
        ("http://100.92.85.92:28990", "http://100.92.85.92:28990"),
        (
            "  http://100.92.85.92:28990/  ",
            "http://100.92.85.92:28990",
        ),
        ("https://Node.Example", "https://node.example"),
        ("http://node.example:80", "http://node.example"),
        ("http://127.0.0.1:8844", "http://127.0.0.1:8844"),
        ("http://localhost:28990", "http://localhost:28990"),
        ("http://[::1]:28990", "http://[::1]:28990"),
    ] {
        assert_eq!(endpoint_origin(typed).as_deref(), Some(origin), "{typed}");
        assert_eq!(canonical_endpoint(typed.to_string()), origin, "{typed}");
    }
    for refused in [
        "",
        "100.92.85.92:28990",
        "ftp://100.92.85.92:28990",
        "http://",
        "http://100.92.85.92:28990/v1",
        "http://100.92.85.92:28990?chain=dognet",
        "http://100.92.85.92:28990#top",
        "http://duck:secret@100.92.85.92:28990",
        "http://100.92.85.92:99999",
    ] {
        assert_eq!(endpoint_origin(refused), None, "{refused:?}");
    }
}

/// A STORED NODE URL IS STILL THE WORKSPACE'S OWN NODE (#92, #70). The URL
/// Settings keeps resolves to the workspace — its keystore, its chain — so the
/// open's own-node check runs against whatever answers there, exactly as on
/// `node.toml`'s port: another network's node, or another key, is refused. The
/// address is no exception for being remote.
#[test]
fn the_own_node_check_holds_a_stored_node_url_to_its_workspace() {
    let (home, key) = joined_workspace("dognet#92");
    let dir = home.path().join("dognet#92");
    let remote = "http://100.92.85.92:28992";
    let mut prefs = serde_json::json!({});
    assert_eq!(
        workspace_serving(&prefs, home.path(), remote),
        None,
        "unset, the address is a remote"
    );
    store_endpoint_override(&mut prefs, "dognet#92", remote);
    let workspace = workspace_serving(&prefs, home.path(), remote);
    assert_eq!(workspace, Some(("dognet#92".to_string(), dir)));

    let status = |chain_id: &str, key: &str| -> serde_json::Value {
        serde_json::from_str(serving_status(chain_id, key)).unwrap()
    };
    assert_eq!(
        own_workspace_node(workspace.clone(), &status("catnet#92", &key)),
        Err("another network's node (catnet#92) answers on this port".to_string())
    );
    assert_eq!(
        own_workspace_node(workspace.clone(), &status("dognet#92", &"bb".repeat(32))),
        Err("a node with another key answers on this port".to_string())
    );
    assert_eq!(
        own_workspace_node(workspace, &status("dognet#92", &key)),
        Ok(())
    );
}

/// SETTINGS KEEPS A NODE RPC URL ONLY WHERE THE WORKSPACE'S OWN NODE ANSWERS
/// (#92, #70). The session reconnects to what a set resolves to, and a
/// reconnect checks nothing — so the set makes the open's check first: a typed
/// value that is not a URL, or a node of another network or another key at
/// it, is refused with its sentence and the prefs stay as they were. A clear
/// goes back to `node.toml` through the same check.
#[tokio::test(flavor = "current_thread")]
async fn a_node_rpc_url_is_kept_only_where_the_workspaces_own_node_answers() {
    let (home, key) = joined_workspace("dognet#93");
    let dir = home.path().join("dognet#93");
    let workspace = || ("dognet#93".to_string(), dir.clone());
    let mut prefs = serde_json::json!({"networks": {"dognet#93": {"last_used": 7}}});
    let before = prefs.clone();

    assert_eq!(
        repoint_workspace(&mut prefs, workspace(), "http://100.92.85.92:28990/v1").await,
        Err("A node RPC URL is http:// or https:// followed by a host and an optional port, and nothing else.".to_string())
    );
    assert_eq!(prefs, before, "a refused URL stores nothing");
    for (status, said) in [
        (
            serving_status("catnet#93", &key),
            "another network's node (catnet#93) answers on this port",
        ),
        (
            serving_status("dognet#93", &"bb".repeat(32)),
            "a node with another key answers on this port",
        ),
    ] {
        let foreign = super::node_that_serves_its_status_once(status).await;
        assert_eq!(
            repoint_workspace(&mut prefs, workspace(), &foreign).await,
            Err(said.to_string())
        );
        assert_eq!(prefs, before, "a foreign node's URL stores nothing");
    }

    let own = super::node_that_serves_its_status_once(serving_status("dognet#93", &key)).await;
    assert_eq!(
        repoint_workspace(&mut prefs, workspace(), &format!(" {own}/ ")).await,
        Ok(EndpointFacts {
            endpoint: own.clone(),
            endpoint_override: own.clone(),
        })
    );
    assert_eq!(
        prefs["networks"]["dognet#93"],
        serde_json::json!({"last_used": 7, "endpoint": own})
    );

    let local = super::node_that_serves_its_status_once(serving_status("dognet#93", &key)).await;
    let node_toml = std::fs::read_to_string(dir.join("node.toml")).unwrap();
    let listen = local.trim_start_matches("http://");
    std::fs::write(
        dir.join("node.toml"),
        node_toml.replace("0.0.0.0:18619", listen),
    )
    .unwrap();
    assert_eq!(
        repoint_workspace(&mut prefs, workspace(), "").await,
        Ok(EndpointFacts {
            endpoint: local,
            endpoint_override: String::new(),
        })
    );
    assert_eq!(prefs, before, "a clear leaves the last-used stamp");
}

/// THE JOIN WAITS HONESTLY (#18). The app attaches to a node it does not
/// supervise, so step 4 never says "starting": every second it names the
/// launcher commands for the workspace, and once patience runs out it goes
/// `blocked` with a line saying the app runs no nodes, still unsettled so
/// the poll goes on.
#[test]
fn a_node_that_never_answers_is_waited_for_by_its_launcher_command() {
    let key = "ab".repeat(32);
    let command = format!(
        "<archive>/ducktape-node-launcher install --workspace '/home/member/.ducktape/dognet#d2a0ec8f' --config '/home/member/.ducktape/dognet#d2a0ec8f/node.toml' --from '<archive>/ducktape' --release-key {key}\n\
         <archive>/ducktape-node-launcher run --workspace '/home/member/.ducktape/dognet#d2a0ec8f' --config '/home/member/.ducktape/dognet#d2a0ec8f/node.toml'"
    );
    let steps: Vec<ProvisionStep> = (1..=PROVISION_PATIENCE + 2)
        .map(|attempts| {
            node_wait_step(
                "/home/member/.ducktape/dognet#d2a0ec8f",
                Some(&key),
                attempts,
                "",
            )
        })
        .collect();
    for step in &steps {
        assert_eq!(step.index, 4);
        assert_eq!(step.label, "Waiting for your node");
        assert_eq!(step.command, command);
        assert!(!step.settled, "the wait keeps polling: {step:?}");
        assert!(!format!("{step:?}").contains("starting"), "{step:?}");
    }
    let (waiting, blocked) = steps.split_at(PROVISION_PATIENCE as usize - 1);
    for step in waiting {
        assert_eq!((step.state.as_str(), step.hint.as_str()), ("waiting", ""));
    }
    for step in blocked {
        assert_eq!(step.state, "blocked");
        assert_eq!(
            step.hint,
            "This app does not run nodes. Run both lines in a terminal; this step continues when the node answers. ducktape-node-launcher ships inside the node release archive, beside ducktape: <archive> is the directory you unpacked that archive into. --release-key pins which release signer this node trusts; installed without it, the node trusts no signer and does not update itself.\n\
             If your archive's launcher is release 2, run with DUCKTAPE_MODULES_DIR='<archive>/modules' set."
        );
    }

    // a quote in the directory cannot end the quoted argument early.
    assert_eq!(
        node_wait_step("/home/o'neil/.ducktape/w#1", Some(&key), 1, "")
            .command
            .lines()
            .nth(1),
        Some(
            r"<archive>/ducktape-node-launcher run --workspace '/home/o'\''neil/.ducktape/w#1' --config '/home/o'\''neil/.ducktape/w#1/node.toml'"
        )
    );
}

/// A FRESH JOIN IS INSTALLED BEFORE IT RUNS, FROM THE ARCHIVE (#46, #33). A join
/// writes no `updates/` tree, and the launcher's `run` refuses a workspace
/// without one (`state_missing`); the launcher is not on `PATH` either, it
/// sits in the node release archive beside `ducktape`. So the step names the
/// launcher's `install` from `<archive>` before its `run`, and the hint says
/// where the launcher comes from. With no key pinned (a bare run) the command
/// cannot print the network's release key, so it names `<release key>` and
/// the hint says whose key that is.
#[test]
fn a_fresh_join_is_installed_from_the_archive_before_it_runs() {
    let step = node_wait_step(
        "/home/member/.ducktape/dognet#d2a0ec8f",
        None,
        PROVISION_PATIENCE,
        "",
    );
    assert_eq!(
        step.command,
        "<archive>/ducktape-node-launcher install --workspace '/home/member/.ducktape/dognet#d2a0ec8f' --config '/home/member/.ducktape/dognet#d2a0ec8f/node.toml' --from '<archive>/ducktape' --release-key <release key>\n\
         <archive>/ducktape-node-launcher run --workspace '/home/member/.ducktape/dognet#d2a0ec8f' --config '/home/member/.ducktape/dognet#d2a0ec8f/node.toml'"
    );
    assert_eq!(step.state, "blocked");
    assert_eq!(
        step.hint,
        "This app does not run nodes. Run both lines in a terminal; this step continues when the node answers. ducktape-node-launcher ships inside the node release archive, beside ducktape: <archive> is the directory you unpacked that archive into. --release-key pins which release signer this node trusts; installed without it, the node trusts no signer and does not update itself. <release key> is the release key your network's operator published.\n\
         If your archive's launcher is release 2, run with DUCKTAPE_MODULES_DIR='<archive>/modules' set."
    );
}

/// A NODE THAT ANSWERS WITH NO MESH IS NOT READY (#41). Its netstack plane
/// failed, so it answers `/v1/status` and still has no overlay for the rest of
/// its boot: the wait does not settle on "Your node answered", it goes
/// `blocked` on the first answer with core's sentence as the hint, and polls on.
#[tokio::test(flavor = "current_thread")]
async fn a_node_whose_netstack_plane_failed_holds_the_wait_with_its_sentence() {
    let rpc = super::node_that_serves_its_status_once(
        r#"{"height":3,"operations":{"phase":"validating","netstack":{"backend":"failed","failure_reason":"netstack_guest_unreadable","failure_detail":"no founding set beside the binary"}}}"#,
    )
    .await;
    let mut steps = provision_progress("no-such-workspace#41".into(), rpc).skip(3);
    let step = steps.next().await.expect("the wait reports");
    assert_eq!(step.index, 4);
    assert_eq!(
        (step.state.as_str(), step.settled),
        ("blocked", false),
        "{step:?}"
    );
    assert_eq!(step.hint, "no founding set beside the binary");
}

/// A NODE THAT ANSWERS BEFORE IT SERVES IS NOT READY (#9). Its HTTP surface is
/// up while it is still `starting`, before it is admitted and before it wires
/// the invite minter the ready screen offers: the wait does not settle on
/// "Your node answered" on that first answer, it names the node's phase, as
/// the launch row does, and polls on. The workspace's own node, serving,
/// settles it.
#[tokio::test(flavor = "current_thread")]
async fn a_node_before_serving_holds_the_wait_on_its_phase() {
    for (status, phase) in [
        (
            r#"{"height":0,"operations":{"phase":"starting"}}"#,
            "starting",
        ),
        (
            r#"{"height":0,"operations":{"phase":"joining"}}"#,
            "joining",
        ),
    ] {
        let rpc = super::node_that_serves_its_status_once(status).await;
        let mut steps = provision_progress("no-such-workspace#9".into(), rpc).skip(3);
        let step = steps.next().await.expect("the wait reports");
        assert_eq!(step.index, 4);
        assert_eq!(
            (step.state.as_str(), step.settled),
            ("waiting", false),
            "{step:?}"
        );
        assert_eq!(step.label, format!("Waiting for your node · {phase}"));
    }
    let (home, key) = joined_workspace("dognet#9");
    let rpc = super::node_that_serves_its_status_once(serving_status("dognet#9", &key)).await;
    let mut steps = provision_progress_in(Some(home.path().into()), "dognet#9".into(), rpc).skip(3);
    let step = steps.next().await.expect("the wait reports");
    assert_eq!((step.index, step.settled), (4, true), "{step:?}");
}

/// A home holding the one workspace a join of `chain_id` wrote — its
/// `network.toml`, `node.toml` and `identity.key` — and the key its node
/// publishes.
fn joined_workspace(chain_id: &str) -> (tempfile::TempDir, String) {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(chain_id);
    std::fs::create_dir_all(&dir).unwrap();
    workspace_config::NetworkDescriptor {
        chain_id: chain_id.into(),
        validators: vec!["aa".repeat(32)],
        bootstrap: vec![],
        reach: vec![],
        coordination: None,
        block_time_ms: workspace_config::DEFAULT_BLOCK_TIME_MS,
        modules: vec![],
        genesis: String::new(),
    }
    .save(&dir.join("network.toml"))
    .unwrap();
    std::fs::write(
        dir.join("node.toml"),
        r#"network = "network.toml"
key_file = "identity.key"
listen = "0.0.0.0:52200"
advertised = "overlay"
storage_dir = "data"
http_listen = "0.0.0.0:18619"
gateway_listen = "127.0.0.1:0"
rpc_listen = "127.0.0.1:18620"
wireguard_listen = "0.0.0.0:51820"
invite_listen = "0.0.0.0:51821"
wireguard_advertised = "auto"
primary_coordinator = "none"
coordinator_relay = "none"
checkpoint_blocks = 32
"#,
    )
    .unwrap();
    let secret = ed25519::PrivateKey::from_seed(70);
    workspace_config::write_identity(&dir.join("identity.key"), &secret).unwrap();
    (
        home,
        workspace_config::hex_bytes(secret.public_key().as_ref()),
    )
}

/// A serving node's status, naming its chain and key.
fn serving_status(chain_id: &str, key: &str) -> &'static str {
    let status = serde_json::json!({
        "height": 3,
        "chain_id": chain_id,
        "public_key": key,
        "operations": {"phase": "serving"},
    });
    Box::leak(status.to_string().into_boxed_str())
}

/// A PORT IS NOT AN IDENTITY (#70). Step 4 polls the endpoint the join wrote,
/// and another network's node, or another node of this one, can be the one
/// answering there. The wait settles only on the workspace's own node — its
/// chain id and the key its `identity.key` holds — and otherwise stays
/// waiting, saying what answered instead.
#[tokio::test(flavor = "current_thread")]
async fn only_the_workspaces_own_node_settles_the_wait() {
    let (home, key) = joined_workspace("dognet#70");
    for (status, said) in [
        (
            serving_status("catnet#70", &key),
            "another network's node (catnet#70) answers on this port",
        ),
        (
            serving_status("dognet#70", &"bb".repeat(32)),
            "a node with another key answers on this port",
        ),
    ] {
        let rpc = super::node_that_serves_its_status_once(status).await;
        let mut steps =
            provision_progress_in(Some(home.path().into()), "dognet#70".into(), rpc).skip(3);
        let step = steps.next().await.expect("the wait reports");
        assert_eq!(step.index, 4);
        assert_eq!(
            (step.state.as_str(), step.settled),
            ("waiting", false),
            "{step:?}"
        );
        assert_eq!(step.hint, said);
    }
    // an answerer that has published neither is not yet anyone: the open
    // refuses it ([`own_node`] `Ok(false)`), never takes it as a match.
    let dir = home.path().join("dognet#70");
    assert_eq!(
        own_node(&dir, "dognet#70", &NodeFacts::default()),
        Ok(false)
    );
    let rpc = super::node_that_serves_its_status_once(serving_status("dognet#70", &key)).await;
    let mut steps =
        provision_progress_in(Some(home.path().into()), "dognet#70".into(), rpc).skip(3);
    let step = steps.next().await.expect("the wait reports");
    assert_eq!((step.index, step.settled), (4, true), "{step:?}");
}

/// THE HTTP CLIENT'S SENTENCE IS EVIDENCE, NOT COPY (#19). A node nothing
/// listened for reached the launch window as "identity query failed: error
/// sending request for url (…/v1/query)". The open path says it in the app's
/// words and keeps the client's in the log — and only when the node is really
/// out of reach: the client gives one reason for every exchange it could not
/// complete, so a node that still answers keeps the failure's own sentence.
#[tokio::test(flavor = "current_thread")]
async fn a_node_nothing_answers_for_is_said_in_the_apps_words_and_logged_in_the_clients() {
    #[derive(Clone, Default)]
    struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let log = Log::default();
    let _logging = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer({
                let log = log.clone();
                move || log.clone()
            })
            .finish(),
    );
    let lookup = |rpc: String| async move {
        rpc_client(&rpc)
            .expect("an http origin")
            .query::<_, identity::IdentityReply>(
                "identity",
                &identity::IdentityQuery::OfKey { key: vec![7; 32] },
            )
            .await
            .expect_err("no identity module answers here")
    };

    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        format!("http://{}", listener.local_addr().expect("its address"))
    };
    let failed = lookup(dead.clone()).await;
    let raw = failed.message().to_string();
    assert_eq!(failed.reason(), "rpc_client");
    let said = exchange_failure(&dead, failed).await;
    assert_eq!(said, format!("Can't reach this network's node at {dead}."));
    let logged = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains("node_unreachable") && logged.contains(&raw),
        "the client's own words are kept in the log: {logged}"
    );

    // a node that answers its status read is reachable, whatever the lookup
    // could not read back from it.
    let answering = node_that_serves_its_status_once(r#"{"height":1}"#).await;
    let failed = lookup(answering.clone()).await;
    let raw = failed.message().to_string();
    let said = exchange_failure(&answering, failed).await;
    assert_eq!(said, raw);
}
