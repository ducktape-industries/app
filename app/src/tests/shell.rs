//! Native app state and WASM view-prop routing regressions.
use super::*;
#[test]
fn a_pushed_status_moves_every_fact_it_carries() {
    let (mut app, _) = Ducktape::boot();
    app.connected = true;

    let _ = app.update(AppMessage::NodeStatusPushed(backend::NodeFacts {
        public_key: "node-key".into(),
        version: "0.2.0".into(),
        contract: backend::EXPECTED_NODE_CONTRACT,
        root_hash: "hash-new".into(),
        chain_id: "mynet#d0cdf950".into(),
        checkpoint_height: 512,
        last_finalized_at: 999,
        height: 888,
        view: Some(9),
        quorum: Some(5),
        reachable_validators: Some(6),
        phase: "syncing".into(),
        phase_since: 1_700_000_000,
        sync_target: 900,
        sync_applied: 412,
        sync_retries: 2,
        sync_failures: 1,
        sync_last_error: "peer hung up".into(),
        network_height: 900,
        behind_by: 12,
        heard_at: 1_700_000_100,
        netstack_failure_reason: String::new(),
        netstack_failure_detail: String::new(),
    }));

    // ALL SEVENTEEN, because a field the handler forgot stays frozen at its
    // connect-time value for as long as the console is open.
    assert_eq!(app.node_key, "node-key");
    assert_eq!(app.node_root_hash, "hash-new");
    assert_eq!(app.network_chain_id, "mynet#d0cdf950");
    assert_eq!(app.node_root_hash, "hash-new");
    assert_eq!(app.node_checkpoint, 512);
    assert_eq!(app.node_last_finalized, 999);
    assert_eq!(app.node_height, 888);
    assert_eq!(app.node_view_label, "9");
    assert_eq!(app.node_quorum_label, "5");
    assert_eq!(app.node_reachable_label, "6");
    assert_eq!(app.node_phase, "syncing");
    assert_eq!(app.node_phase_since, 1_700_000_000);
    assert_eq!(app.node_sync_target, 900);
    assert_eq!(app.node_sync_applied, 412);
    assert_eq!(app.node_sync_retries, 2);
    assert_eq!(app.node_sync_failures, 1);
    assert_eq!(app.node_sync_last_error, "peer hung up");
}

/// THE EXPLORER DRAWS THE LIVE REGISTER, NOT ITS OWN NEWEST ROW.
///
/// Its list is op-carrying blocks only, so the top row lags the chain by
/// however many idle blocks have passed — on a quiet chain, forever. The head
/// a reader watches moves on the ws heartbeat, every block, nop fillers
/// included, and the screen was not even handed it: it had a hundred-block
/// snapshot and a refresh button.

#[test]
fn shell_tab_is_app_state_and_an_opened_message_switches_panes() {
    let (mut app, _) = Ducktape::boot();
    assert_eq!(app.shell_tab, ShellTab::View("chat"));
    let _ = app.update(AppMessage::SelectShellTab(ShellTab::View("pages")));
    assert_eq!(app.shell_tab, ShellTab::View("pages"));

    // a `duck://` message address lands on the chat pane, whichever view
    // handed it to the open plane
    app.loading = false;
    app.mutation_phase = MutationPhase::Idle;
    app.connected_rpc = "http://node".into();
    let _ = app.update(AppMessage::OpenChatSearchHit("general".into(), 7));
    assert_eq!(app.shell_tab, ShellTab::View("chat"));
}

/// Node operations are an operator surface, not a tail appended to device
/// preferences. Pin all three routing seams so a visual reshuffle cannot bury
/// the screen in Settings again while leaving its handlers intact.

#[test]
fn switching_panes_retires_a_stale_error_banner_on_every_tab() {
    // the disconnected path returns first, and must still clear.
    let (mut app, _) = Ducktape::boot();
    app.error = "could not reach the node".into();
    let _ = app.update(AppMessage::SelectShellTab(ShellTab::View("files")));
    assert_eq!(
        app.error, "",
        "the !connected early return must still clear"
    );

    // the chat/pages path returns second, and must still clear.
    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    app.error = "files: path not found".into();
    let _ = app.update(AppMessage::SelectShellTab(ShellTab::View("pages")));
    assert_eq!(
        app.error, "",
        "the chat/pages early return must still clear"
    );

    // and the full path, which falls through to the generation bumps.
    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    app.error = "explorer hydration failed".into();
    let _ = app.update(AppMessage::SelectShellTab(ShellTab::View("members")));
    assert_eq!(app.error, "");
    assert_eq!(app.shell_tab, ShellTab::View("members"));
}

/// THE RAIL'S ACCOUNT ROW NEVER SENDS A SIGNED-IN USER TO THE SIGN-IN.
///
/// The row reads "who is signed in"; pressing it opens the account screen —
/// Settings, which draws the account card — and only a session with NO
/// account goes to the welcome window. The welcome window replaces the
/// console, so routing a signed-in press there closes the console under the
/// account it was asked to show: the re-login the row shipped with.
#[test]
fn the_rail_account_row_opens_settings_when_signed_in_and_the_welcome_when_not() {
    use futures::StreamExt as _;
    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    app.account_exists = true;
    app.shell_tab = ShellTab::View("chat");
    let task = app.update(AppMessage::OpenAccount);
    let queued = futures::executor::block_on(task.into_stream().collect::<Vec<_>>());
    assert!(
        matches!(
            queued.as_slice(),
            [AppMessage::SelectShellTab(ShellTab::View("settings"))]
        ),
        "a signed-in press goes to the account card in Settings"
    );
    assert_eq!(
        app.hub_chain_id, "",
        "a signed-in press never arms the welcome window"
    );

    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    app.account_exists = false;
    app.network_chain_id = "mynet#d0cdf950".into();
    let task = app.update(AppMessage::OpenAccount);
    let queued = futures::executor::block_on(task.into_stream().collect::<Vec<_>>());
    assert!(
        queued.is_empty(),
        "a press with no account opens the welcome window, not a tab"
    );
    assert_eq!(
        app.hub_chain_id, "mynet#d0cdf950",
        "the welcome window is armed with this network"
    );
}

/// EVERY READER OF `/v1/peers` USES THE NAMES `PeerView` SERIALIZES.
///
/// `crates/noded/src/peers.rs` serves `peer` / `connected` / `role`; it has never
/// served `key`, `live`, or a per-peer `height`. Reading the wrong ones does
/// not fail — `as_str()` answers `None` and the row renders blank, zero and
/// offline for a peer that is connected.
///
/// This has already happened twice in two different readers: `roster.rs`
/// carries the scar in a comment, and Settings' PEERS table shipped with all
/// three wrong names. The app cannot depend on `noded` to pin the contract with
/// a type, so it is pinned here instead — one rule over every reader the app
/// still holds. The Node view reads `/v1/peers` through the kernel now and
/// carries the same guard over its own source (`crates/views/node/tests`).

#[test]
fn a_move_to_a_pane_that_does_not_draw_the_settings_facts_keeps_the_connect_load() {
    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    let in_flight = app.settings_generation;

    let _ = app.update(AppMessage::SelectShellTab(ShellTab::View("members")));
    let _ = app.update(AppMessage::SettingsLoaded(crate::backend::SettingsFacts {
        generation: in_flight,
        key_path: "/w/user.key".into(),
        key_state: "encrypted".into(),
        data_dir: "/w".into(),
        user_key: "abcd".into(),
        endpoint: Default::default(),
    }));
    assert_eq!(
        app.settings_user_key, "abcd",
        "the move off-tab must not revoke the connect load's facts"
    );

    // and the tab that DOES draw them still re-reads on entry.
    let _ = app.update(AppMessage::SelectShellTab(ShellTab::View("settings")));
    assert_ne!(
        app.settings_generation, in_flight,
        "entering Settings must issue a fresh read"
    );
}

/// A WORKSPACE'S NODE RPC URL IS SETTINGS' TO DRAW AND TO SET (#92). The view
/// sends `endpoint` with the typed `url`; the app draws the URL in use, the
/// stored override and — after a refused set, until the next kept one — the
/// refusal's one sentence. A kept URL (or a clear) re-points the session and
/// reconnects it the way Settings → Reconnect does.
#[test]
fn settings_draws_the_node_rpc_url_and_a_kept_one_reconnects_the_session() {
    let (mut app, _) = Ducktape::boot();
    app.shell_tab = ShellTab::View("settings");
    let drawn = |app: &Ducktape| -> [serde_json::Value; 3] {
        let (view, _) = app.native_view();
        let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
        [
            "rpc_endpoint",
            "rpc_endpoint_override",
            "rpc_endpoint_refusal",
        ]
        .map(|fact| props[fact].clone())
    };
    let editability = |app: &Ducktape| -> [serde_json::Value; 2] {
        let (view, _) = app.native_view();
        let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
        [
            props["rpc_endpoint_editable"].clone(),
            props["rpc_endpoint_editability_reason"].clone(),
        ]
    };
    let event = crate::module_view::view_event(
        "endpoint".into(),
        r#"{"url":"http://100.92.85.92:28990"}"#.into(),
    );
    assert_eq!(
        crate::module_view::settings_intent(&event),
        SettingsIntent::Endpoint
    );
    assert_eq!(
        crate::module_view::event_text(&event, "url"),
        "http://100.92.85.92:28990"
    );

    let _ = app.update(AppMessage::SettingsLoaded(crate::backend::SettingsFacts {
        generation: app.settings_generation,
        key_path: "/w/user.key".into(),
        key_state: "encrypted".into(),
        data_dir: "/w".into(),
        user_key: "abcd".into(),
        endpoint: crate::backend::EndpointFacts {
            endpoint: "http://127.0.0.1:8844".into(),
            endpoint_override: String::new(),
        },
    }));
    assert_eq!(drawn(&app), ["http://127.0.0.1:8844", "", ""]);
    assert_eq!(
        editability(&app),
        [serde_json::json!(true), serde_json::json!("")]
    );

    let refusal = "A node RPC URL is http:// or https:// followed by a host and an optional port, and nothing else.";
    let _ = app.update(AppMessage::SettingsEndpointRefused(
        refusal.to_string().into(),
    ));
    assert_eq!(drawn(&app), ["http://127.0.0.1:8844", "", refusal]);

    let remote = "http://100.92.85.92:28990";
    let _ = app.update(AppMessage::SettingsEndpointSaved(
        crate::backend::EndpointFacts {
            endpoint: remote.into(),
            endpoint_override: remote.into(),
        },
    ));
    assert_eq!(drawn(&app), [remote, remote, ""]);
    assert_eq!(app.connected_rpc, remote, "the session is re-pointed");
    assert!(
        handler_body("SettingsEndpointSaved").contains("Task::done(AppMessage::Reconnect)"),
        "a kept URL reconnects through Settings → Reconnect's own message"
    );

    let _ = app.update(AppMessage::SettingsEndpointSaved(
        crate::backend::EndpointFacts {
            endpoint: "http://127.0.0.1:8844".into(),
            endpoint_override: String::new(),
        },
    ));
    assert_eq!(drawn(&app), ["http://127.0.0.1:8844", "", ""]);

    let _ = app.update(AppMessage::SettingsLoaded(crate::backend::SettingsFacts {
        generation: app.settings_generation,
        key_path: "/w/user.key".into(),
        key_state: "encrypted".into(),
        data_dir: String::new(),
        user_key: "abcd".into(),
        endpoint: crate::backend::EndpointFacts {
            endpoint: remote.into(),
            endpoint_override: String::new(),
        },
    }));
    assert_eq!(drawn(&app), [remote, "", ""]);
    assert_eq!(
        editability(&app),
        [
            serde_json::json!(false),
            serde_json::json!("This connection has no local workspace."),
        ]
    );
}

/// THE JOIN OPENS THE CALL'S WINDOW, AND THE CONSOLE KEEPS SAYING SO.
///
/// A huddle has exactly one surface — its own window — so an ack that opened
/// none would leave someone in a live call with nowhere to see it. The route is
/// pinned here because a join routed back to the generic `chat_acked` would
/// land the same silence.
///
/// THIS USED TO BE THE OPPOSITE ASSERTION, and the defect it guarded is worth
/// restating because it is what the two lines below now answer: a second OS
/// window fell behind the console the moment anything in the console was
/// clicked, and the console said nothing about the call at all. So the window
/// uses a native popup window and the channel's LIVE pill draws
/// whenever that channel's call is live rather than only while the window is
/// up — the pill is the way back to a window someone has closed.

#[test]
fn onboarding_capabilities_are_secret_buffers_cleared_on_navigation() {
    let (mut app, _) = Ducktape::boot();
    let recovery = "duck ".repeat(24);
    let invite = "duck-capability".to_string();

    let _ = app.update(AppMessage::SecretTyped(
        "restore_words".into(),
        recovery.clone(),
    ));
    let _ = app.update(AppMessage::SecretTyped(
        "join_invite".into(),
        invite.clone(),
    ));
    assert_eq!(app.secrets.text("restore_words"), recovery);
    assert_eq!(app.secrets.text("join_invite"), invite);
    let snapshot = format!("{app:?}");
    assert!(!snapshot.contains("duck-capability"));
    assert!(!snapshot.contains("duck duck"));

    let _ = app.update(AppMessage::GoNetworks);
    assert!(app.secrets.text("restore_words").is_empty());
    assert!(app.secrets.text("join_invite").is_empty());
}

#[test]
fn ready_events_rehydrate_without_rewinding_the_tip() {
    let (mut live, _) = Ducktape::boot();
    live.loading = false;
    live.block_height = 41;
    live.hydration_generation = 2;
    let _ = live.update(AppMessage::LiveUpdated(backend::LiveUpdate {
        kind: LiveKind::Ready,
        status: "Live".into(),
        height: -1,
        load_chat: true,
        ..backend::LiveUpdate::default()
    }));
    assert_eq!(
        live.hydration_generation, 3,
        "ready starts the subscribe-then-hydrate catch-up resync"
    );
    assert_eq!(live.block_height, 41, "a heightless event keeps the tip");
}

/// "NOTHING OPEN" IS TWO DIFFERENT FACTS AND THE PLATE MUST NOT CONFLATE THEM.
/// One message served both, so a workspace that had never held a vote read
/// `0 open · 0 settled` in its header and "every decision on this network is
/// finalized" in its body — asserting a history of decisions nobody ever made.
/// Driven on the running app: the demo network shows exactly that.
///
/// Pinned as COMPLEMENTARY CONDITIONS, not as copy. Asserting the sentences
/// alone would stay green if both arms fired at once, or if the new arm were
/// unreachable.

#[test]
fn a_tab_move_retires_the_banner_of_the_screen_it_left() {
    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    app.shell_tab = ShellTab::View("chat");
    app.error = "the room would not load".into();

    let _ = app.update(AppMessage::SelectShellTab(ShellTab::View("node")));

    assert_eq!(app.error, "", "a banner never rides a tab move");
    assert_eq!(app.shell_tab, ShellTab::View("node"));

    // The chat/pages return and the disconnected return each skip the
    // generation bumps below, and neither may keep a stale banner alive.
    let (mut app, _) = Ducktape::boot();
    app.shell_tab = ShellTab::View("pages");
    app.error = "the page would not load".into();
    let _ = app.update(AppMessage::SelectShellTab(ShellTab::View("chat")));
    assert_eq!(app.error, "");
    assert_eq!(app.shell_tab, ShellTab::View("chat"));
}

/// THE FIVE IDENTITY OPS LAND IN ONE PLACE. `account_changed` is the only
/// handler that re-reads the account for them, and it frees the card and
/// drops the ticket: one left on screen after its device joined is a stale
/// blob that looks like a secret. Which DRAFTS the op spent is the Settings
/// view's own reading of the facts that moved — the kernel holds none of
/// them.

#[test]
fn a_committed_identity_op_rereads_the_account_and_frees_the_card() {
    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    app.connected_rpc = "http://node".into();
    app.account_busy = true;
    app.account_ticket = "{}".into();
    let before = app.account_generation;

    let _ = app.update(AppMessage::AccountChanged(true));

    assert!(!app.account_busy, "the op is over");
    assert_eq!(app.account_generation, before + 1, "the account is re-read");
    assert!(app.account_ticket.is_empty());
}

/// THE BROWSER CEREMONIES ARE WIRED LIKE THE PASTED OPS: each button emits
/// its own signal, the Settings view sends it as an intent, and each intent's
/// arm runs its backend fn on the connected chain under the signing seat,
/// landing in `account_changed` / `account_op_failed` — the one pair that
/// re-reads the account and frees the card. And each is offered only where
/// consensus would accept it: registering/linking with an account, logging in
/// without one.

#[test]
fn a_minted_ticket_is_shown_without_a_reread() {
    let (mut app, _) = Ducktape::boot();
    app.account_busy = true;
    let before = app.account_generation;

    let _ = app.update(AppMessage::AccountTicketMinted(r#"{"add_key":{}}"#.into()));

    assert!(!app.account_busy);
    assert_eq!(app.account_ticket, r#"{"add_key":{}}"#);
    assert_eq!(app.account_generation, before, "minting re-reads nothing");
}

/// THE CARD OFFERS ONLY WHAT CONSENSUS WOULD ACCEPT: founding only while there
/// is no account, and never the removal of the last key (the module refuses
/// it, and a button that always refuses is a lie).

#[test]
fn the_explorer_is_handed_the_live_head_and_the_phase() {
    let mut app = Ducktape::initial_state();
    app.shell_tab = ShellTab::View("explorer");
    app.block_height = 1234;
    app.node_phase = "syncing".into();
    app.node_sync_applied = 30;
    app.node_sync_target = 40;
    let (view, _) = app.native_view();
    assert_eq!(view.module, "explorer");
    let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
    assert_eq!(props["head"], 1234);
    assert_eq!(props["sync_line"], backend::sync_label("syncing", 30, 40));
}
#[test]
fn no_seat_prints_a_checkpoint_beside_the_live_head() {
    let mut app = Ducktape::initial_state();
    app.shell_tab = ShellTab::View("node");
    app.node_height = 100;
    app.node_checkpoint = 90;
    app.block_height = 200;
    let (view, _) = app.native_view();
    let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
    assert!(props.get("block_height").is_none());
    assert!(
        props.get("checkpoint").is_none(),
        "node guest owns both checkpoint and sampled head"
    );
}
#[test]
fn the_node_streams_carry_the_gates_their_costs_require() {
    let source = rust_tokens(include_str!("../ui/app.rs"));
    assert_eq!(
        source.matches("crate::backend::node_status_live(").count(),
        1
    );
    assert_eq!(source.matches("crate::backend::live_events(").count(), 1);
    for removed in [
        "node_peers_live(",
        "node_logs(",
        "load_peers(",
        "load_modules(",
    ] {
        assert!(
            !source.contains(removed),
            "{removed} belongs to the node guest"
        );
    }
}
#[test]
fn node_operations_are_a_first_class_screen() {
    let mut app = Ducktape::initial_state();
    app.shell_tab = ShellTab::View("node");
    assert_eq!(app.native_view().0.module, "node");
    app.shell_tab = ShellTab::View("settings");
    assert_eq!(app.native_view().0.module, "settings");
}
#[test]
fn joining_a_huddle_opens_the_call_window() {
    let mut app = Ducktape::initial_state();
    app.active_channel = "general".into();
    app.active_channel_name = "General".into();
    app.huddle_now = 123;
    let _ = app.update(AppMessage::HuddleJoinedAck(true));
    assert!(app.huddle_joined);
    assert_eq!(app.huddle_channel, "general");
    assert_eq!(app.huddle_channel_name, "General");
    assert_eq!(app.huddle_joined_at, 123);
    // the ack and a voice-room join seat the reader through one helper, and
    // it is the helper that summons the window
    let ack = handler_body("HuddleJoinedAck");
    assert!(ack.contains("seat_in_huddle("));
    let seat = fn_body("seat_in_huddle");
    assert!(seat.contains("ShowHuddle"));
    assert!(!seat.contains("shell::open("));
    let joined = handler_body("VoiceJoined");
    assert!(joined.contains("seat_in_huddle("));
}
#[test]
fn a_failed_huddle_leave_keeps_the_retained_roster_visible() {
    // leaving and moving rooms forget the call through one helper
    let leave = handler_body("LeaveHuddleHere");
    assert!(leave.contains("drop_call_state("));
    assert!(handler_body("JoinVoice").contains("drop_call_state("));
    let dropped = fn_body("drop_call_state");
    assert!(dropped.contains("self.call_peers="));
    assert!(!dropped.contains("self.huddle_roster="));
    let ack = handler_body("HuddleLeft");
    for field in ["huddle_joined", "huddle_roster", "huddle_channel"] {
        assert!(ack.contains(&format!("self.{field}=")));
    }
}
#[test]
fn interaction_state_stays_with_the_screen_that_owns_it() {
    let mut app = Ducktape::initial_state();
    for (tab, module) in [
        (ShellTab::View("pages"), "pages"),
        (ShellTab::View("chat"), "chat"),
        (ShellTab::View("files"), "files"),
        (ShellTab::View("agents"), "agents"),
        (ShellTab::View("forge"), "forge"),
        (ShellTab::View("explorer"), "explorer"),
    ] {
        app.shell_tab = tab;
        let (view, _) = app.native_view();
        assert_eq!(view.module, module);
        let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
        for forbidden in [
            "document",
            "editor",
            "history",
            "expanded_nodes",
            "scroll_offset",
        ] {
            assert!(
                props.get(forbidden).is_none(),
                "{module}: guest-local {forbidden}"
            );
        }
    }
}
#[test]
fn passkey_ceremony_props_reach_settings_without_exposing_secrets() {
    let mut app = Ducktape::initial_state();
    app.shell_tab = ShellTab::View("settings");
    app.password = "never-in-props".into();
    app.account_ceremony_phase = "working".into();
    let (view, _) = app.native_view();
    let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
    assert_eq!(props["unlocked"], true);
    assert_eq!(props["account_ceremony_phase"], "working");
    assert!(
        !String::from_utf8(view.props)
            .unwrap()
            .contains("never-in-props")
    );
}
/// THE UPDATE PLANE THROUGH THE SHELL: the Settings props carry the
/// updater's facts (or `unavailable` for a bare `make dev` binary), each
/// Settings intent lands as its own `UpdateAction`, the first window opened
/// is the healthy signal that settles a flipped release, and the console
/// strip draws only the two phases that ask something of the reader.
#[test]
fn update_facts_reach_settings_and_each_intent_is_one_action() {
    use crate::backend::update::{UpdatePaths, Updater};
    use app_update::{Phase, Sha, TrustedKeys};
    use commonware_cryptography::{Signer as _, ed25519};

    let mut app = Ducktape::initial_state();
    app.shell_tab = ShellTab::View("settings");
    let (view, _) = app.native_view();
    let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
    assert_eq!(
        props["update_state"], "unavailable",
        "a bare binary has no updater"
    );
    assert_eq!(props["update_channel"], "stable");
    assert_eq!(app.update_strip(), None);

    let updates = tempfile::tempdir().unwrap();
    let keys = TrustedKeys {
        pinned: app_update::PublicKey::of(&ed25519::PrivateKey::from_seed(1)),
        successor: None,
    };
    let current = Sha::digest(b"new");
    let previous = Sha::digest(b"old");
    let pending = Phase::PendingHealthy(app_update::PendingHealthy {
        current,
        previous,
        boots: 1,
        pinned_sequence: 4,
    });
    app.updater = Some(Updater::new(
        pending,
        Some(keys),
        UpdatePaths::under(updates.path()),
    ));
    let (view, _) = app.native_view();
    let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
    assert_eq!(props["update_state"], "pending_healthy");
    assert_eq!(props["update_current"], current.short());
    assert_eq!(props["update_previous"], previous.short());
    assert_eq!(props["update_checked"], "never");

    // the first window is the healthy signal
    let _ = app.update(AppMessage::OnboardingOpened(
        crate::shell::WindowKey::unique(),
    ));
    let reading = app.update_reading().expect("armed");
    assert_eq!(
        reading.phase,
        Phase::Idle(app_update::Idle {
            current,
            previous: Some(previous),
            pinned_sequence: 4,
        })
    );
    let (view, _) = app.native_view();
    let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
    assert_eq!(props["update_state"], "idle");

    // each Settings intent is one action; the rollback intent leaves the
    // machine alone here because there is no launcher to hand over to
    // (the exec fails and the phase stays what state.json holds).
    for (kind, action) in [
        ("update_check", UpdateAction::CheckNow),
        ("update_restart", UpdateAction::RestartToUpdate),
        ("update_rollback", UpdateAction::RollBack),
    ] {
        let event = crate::module_view::view_event(kind.into(), "{}".into());
        let intent = crate::module_view::settings_intent(&event);
        let routed = match intent {
            SettingsIntent::UpdateCheck => UpdateAction::CheckNow,
            SettingsIntent::UpdateRestart => UpdateAction::RestartToUpdate,
            SettingsIntent::UpdateRollback => UpdateAction::RollBack,
            other => panic!("{kind} routed to {other:?}"),
        };
        assert_eq!(routed, action);
    }
    let body = handler_body("UpdateAction");
    for event in ["RestartToUpdate", "UserRollback", "DismissRollbackNotice"] {
        assert!(
            body.contains(&format!("app_update::Event::{event}")),
            "the action handler feeds {event}"
        );
    }
    assert!(body.contains("updater.check_now(self.wall_now)"));
    assert!(
        body.contains("updater.take_relaunch()") && body.contains("AppMessage::TrayQuit"),
        "a spawned launcher is followed by the app's own shutdown, never an exec"
    );
    let update = rust_tokens(include_str!("../backend/update.rs"));
    assert!(!update.contains(".exec()"), "the app never execs in place");
    assert!(update.contains(".process_group(0).spawn()"));
    let arms = handler_bodies()
        .into_iter()
        .find(|(name, _)| name == "SettingsViewEvent")
        .map(|(_, body)| body)
        .expect("the settings event handler");
    for action in ["CheckNow", "RestartToUpdate", "RollBack"] {
        assert!(
            arms.contains(&format!("UpdateAction::{action}")),
            "the settings event handler routes {action}"
        );
    }

    // Check now starts the fetch and is refused while it runs
    let _ = app.update(AppMessage::UpdateAction(UpdateAction::CheckNow));
    assert!(app.update_reading().unwrap().busy, "a fetch is in flight");
    let (view, _) = app.native_view();
    let props: serde_json::Value = serde_json::from_slice(&view.props).unwrap();
    assert_eq!(props["update_busy"], true);
    let _ = app.update(AppMessage::UpdateJobReplied(None));
    assert!(!app.update_reading().unwrap().busy);

    // the strip: Staged offers the restart, RolledBack the dismissal
    let staged = Phase::Staged(app_update::Staged {
        current,
        previous: Some(previous),
        pinned_sequence: 5,
        staged: Sha::digest(b"next"),
        sequence: 5,
        display: "2026.09.3+abcdef0".into(),
        node_contract: 1,
        refused: None,
    });
    app.updater = Some(Updater::new(
        staged,
        app.updater.as_ref().unwrap().keys().cloned(),
        UpdatePaths::under(updates.path()),
    ));
    assert_eq!(
        app.update_strip(),
        Some(crate::backend::update::UpdateStrip::Ready {
            display: "2026.09.3+abcdef0".into()
        })
    );
    let rolled_back = Phase::RolledBack(app_update::RolledBack {
        current,
        failed: Sha::digest(b"next"),
        reason: app_update::RollbackReason::NeverRendered,
        pinned_sequence: 5,
    });
    app.updater = Some(Updater::new(
        rolled_back,
        app.updater.as_ref().unwrap().keys().cloned(),
        UpdatePaths::under(updates.path()),
    ));
    assert_eq!(
        app.update_strip(),
        Some(crate::backend::update::UpdateStrip::RolledBack {
            failed: Sha::digest(b"next").short(),
            reason: "it never came up".into()
        })
    );
    let _ = app.update(AppMessage::UpdateAction(
        UpdateAction::DismissRollbackNotice,
    ));
    assert_eq!(app.update_strip(), None);
    assert!(matches!(
        app.update_reading().unwrap().phase,
        Phase::Idle(_)
    ));

    // the console draws the strip above the content, wired to the two actions
    let shell = rust_tokens(include_str!("../shell.rs"));
    assert!(shell.contains("letupdate_strip=state.update_strip();"));
    assert!(shell.contains("Message::UpdateAction(crate::UpdateAction::RestartToUpdate)"));
    assert!(shell.contains("Message::UpdateAction(crate::UpdateAction::DismissRollbackNotice)"));
}

/// A launch-window row on this device whose live node speaks a contract the
/// app refuses (#101).
pub(super) fn refused_workspace_row(id: &str, endpoint: &str) -> backend::HubNetwork {
    backend::HubNetwork {
        id: id.into(),
        chain_id: id.into(),
        name: "walk".into(),
        endpoint: endpoint.into(),
        endpoint_override: String::new(),
        kind: "local".into(),
        last_used: 0,
        probed: true,
        live: true,
        height: 2033,
        contract: backend::EXPECTED_NODE_CONTRACT + 1,
        another_network: false,
        phase: "serving".into(),
        behind_by: 0,
        netstack_failure: String::new(),
    }
}

/// An armed updater in `phase`, its files under `dir`.
pub(super) fn armed_updater(
    phase: app_update::Phase,
    dir: &std::path::Path,
) -> crate::backend::update::Updater {
    use commonware_cryptography::{Signer as _, ed25519};
    let keys = app_update::TrustedKeys {
        pinned: app_update::PublicKey::of(&ed25519::PrivateKey::from_seed(1)),
        successor: None,
    };
    crate::backend::update::Updater::new(
        phase,
        Some(keys),
        crate::backend::update::UpdatePaths::under(dir),
    )
}

/// THE UPDATE CHECK NEEDS NO CONSOLE (#101). With no console open the wall
/// tick checks through the selected workspace row's own node even when its
/// contract refuses the console — that refusal is what a newer release
/// lifts. No selection, or a row another network answers for, carries
/// nothing and nothing is checked.
#[test]
fn the_wall_tick_checks_through_a_refused_rows_node_without_a_console() {
    let updates = tempfile::tempdir().unwrap();
    let id = "walk#0e1b62f1";
    let endpoint = "http://127.0.0.1:1";
    let mut app = Ducktape::initial_state();
    app.updater = Some(armed_updater(
        app_update::Phase::Idle(app_update::Idle {
            current: app_update::Sha::digest(b"installed"),
            previous: None,
            pinned_sequence: 2,
        }),
        updates.path(),
    ));
    assert!(!app.connected && app.console_win.is_none());
    app.hub_networks = vec![refused_workspace_row(id, endpoint)];

    // neither a console nor a selection: no carrier, no check
    let _ = app.update(AppMessage::WallTick);
    assert_eq!(app.update_carrier(), None);
    assert!(!app.update_reading().unwrap().busy);

    // a row where another network answers is never the carrier
    app.hub_networks[0].another_network = true;
    app.hub_networks[0].live = false;
    app.hub_selected = id.into();
    assert!(backend::selected_network_refuses(&app.hub_networks, id));
    let _ = app.update(AppMessage::WallTick);
    assert_eq!(app.update_carrier(), None);
    assert!(!app.update_reading().unwrap().busy);

    // the refused row's own node is: the tick starts the fetch through it
    app.hub_networks = vec![refused_workspace_row(id, endpoint)];
    assert!(backend::selected_network_refuses(&app.hub_networks, id));
    assert_eq!(app.update_carrier().as_deref(), Some(endpoint));
    let _ = app.update(AppMessage::WallTick);
    assert!(app.update_reading().unwrap().busy, "a fetch is in flight");

    // every job runs through the carrier, not the console's session alone
    let jobs = rust_tokens(include_str!("../ui/app_update.rs"));
    assert!(jobs.contains("run_job(self.update_carrier().unwrap_or_default(),"));
    assert!(jobs.contains("updater.tick(self.wall_now,carrier)"));
}

/// A REFUSED ROW CARRIES THE OFFER (#101). The launch window draws the
/// console's update strip under a row whose contract refuses the console: a
/// staged release offers Restart to update there, and with nothing staged
/// the strip offers the check itself. A row that opens draws no strip.
#[test]
fn a_refused_row_offers_the_update_on_the_launch_window() {
    use gpui_kit::test::TestWindowExt as _;
    let updates = tempfile::tempdir().unwrap();
    let id = "walk#0e1b62f1";
    let current = app_update::Sha::digest(b"installed");
    let launch_window = |phase: app_update::Phase, contract: u32| {
        let mut app = Ducktape::initial_state();
        app.hub_step = HubStep::Networks;
        app.hub_networks = vec![backend::HubNetwork {
            contract,
            ..refused_workspace_row(id, "http://127.0.0.1:1")
        }];
        app.hub_selected = id.into();
        app.updater = Some(armed_updater(phase, updates.path()));
        onboarding_window(app)
    };
    let staged = app_update::Phase::Staged(app_update::Staged {
        current,
        previous: None,
        pinned_sequence: 2,
        staged: app_update::Sha::digest(b"next"),
        sequence: 3,
        display: "2026.09.3+abcdef0".into(),
        node_contract: backend::EXPECTED_NODE_CONTRACT + 1,
        refused: None,
    });
    let idle = app_update::Phase::Idle(app_update::Idle {
        current,
        previous: None,
        pinned_sequence: 2,
    });
    let refusing = backend::EXPECTED_NODE_CONTRACT + 1;

    let (mut native, window, _) = launch_window(staged.clone(), refusing);
    window
        .update(&mut native, |_, window, cx| {
            window.render_frame(cx);
            let strip = window.within("update-strip");
            assert!(strip.find("update-restart").visible());
            assert!(strip.try_find("update-check").is_none());
        })
        .unwrap();

    let (mut native, window, view) = launch_window(idle, refusing);
    window
        .update(&mut native, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("update-restart").is_none());
            assert!(window.within("update-strip").find("update-check").visible());
            window.click("update-check", cx);
            let reading = view.read(cx).test_state(cx).update_reading().unwrap();
            assert!(reading.last_check.is_some(), "Check for updates checks");
        })
        .unwrap();

    let (mut native, window, _) = launch_window(staged, backend::EXPECTED_NODE_CONTRACT);
    window
        .update(&mut native, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("update-strip").is_none());
        })
        .unwrap();
}

/// A LONG ROW KEEPS ITS WAY OUT (#105). The row's label never shrank below
/// its text, so a long one pushed Use node.toml and Forget out of the launch
/// window. That happened exactly when they were needed: on an override that
/// another network answers, where the console does not open. The label
/// truncates, and the actions stay inside the window.
#[test]
fn a_long_row_label_keeps_its_actions_in_the_launch_window() {
    use gpui_kit::test::TestWindowExt as _;
    let id = "c6scratch-staging-east#0e1b62f1";
    let mut app = Ducktape::initial_state();
    app.hub_step = HubStep::Networks;
    app.hub_networks = vec![backend::HubNetwork {
        name: "c6scratch-staging-east".into(),
        endpoint_override: "http://127.0.0.1:29489".into(),
        live: false,
        another_network: true,
        ..refused_workspace_row(id, "http://127.0.0.1:29489")
    }];
    app.hub_selected = id.into();
    let (mut native, window, _) = onboarding_window(app);
    window
        .update(&mut native, |_, window, cx| {
            window.render_frame(cx);
            let width = window.viewport_size().width;
            for action in ["node-toml", "forget"] {
                let bounds = window.find(format!("{action}/{id}")).bounds();
                assert!(
                    bounds.left() >= gpui_kit::px(0.) && bounds.right() <= width,
                    "{action} {bounds:?} leaves the {width:?} window"
                );
            }
        })
        .unwrap();
}

/// A launch-window row whose node the probe measured silent: after a re-found
/// (#137) every install still holds the dead chain's row.
pub(super) fn silent_workspace_row(id: &str) -> backend::HubNetwork {
    backend::HubNetwork {
        name: "dognet".into(),
        live: false,
        height: -1,
        contract: 0,
        phase: String::new(),
        behind_by: -1,
        ..refused_workspace_row(id, "http://127.0.0.1:1")
    }
}

/// A SAVED NETWORK THAT STOPPED ANSWERING (#137). A network re-founded under
/// a new chain id leaves every install's row for the old one saying only
/// offline. The row says the network has not answered and what to ask for —
/// words true of an unreachable node too, since the app cannot tell the two
/// apart — and offers Join with a new invite beside Forget, on the row. A row
/// whose node answers carries neither.
#[test]
fn a_silent_network_row_says_so_and_offers_a_new_invite() {
    use gpui_kit::test::TestWindowExt as _;
    let (dead, live) = ("dognet#b5b6ea90", "dognet#88507a8b");
    let mut app = Ducktape::initial_state();
    app.hub_step = HubStep::Networks;
    app.hub_networks = vec![
        silent_workspace_row(dead),
        backend::HubNetwork {
            name: "dognet".into(),
            contract: backend::EXPECTED_NODE_CONTRACT,
            ..refused_workspace_row(live, "http://127.0.0.1:2")
        },
    ];
    assert_eq!(
        backend::not_answering_line(&app.hub_networks[0]).as_deref(),
        Some("dognet has not answered. If it was re-founded, ask a member for a new invite.")
    );
    assert_eq!(backend::not_answering_line(&app.hub_networks[1]), None);
    let (mut native, window, view) = onboarding_window(app);
    window
        .update(&mut native, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find(format!("forget/{dead}")).visible());
            assert!(window.find(format!("new-invite/{dead}")).visible());
            assert!(window.try_find(format!("new-invite/{live}")).is_none());
            window.click(format!("new-invite/{dead}"), cx);
            window.render_frame(cx);
            assert_eq!(view.read(cx).test_state(cx).hub_step, HubStep::Join);
            assert!(window.find("join-submit").visible());
        })
        .unwrap();
}

/// `Rendered` is the launcher's contract, not the release channel's: an app
/// the launcher started (`DUCKTAPE_RELEASE` + `DUCKTAPE_UPDATE_STATE`) on an
/// install that pins no release key still settles a flipped release when
/// its first window opens, and only the channel stays off. The env is the
/// process's, so the app runs in a child of this test binary.
#[test]
fn a_launched_app_without_a_release_key_still_reports_it_rendered() {
    use app_update::{Idle, PendingHealthy, Phase, Sha, state};

    let root = tempfile::tempdir().unwrap();
    let updates = root.path().join("cfg/ducktape/updates");
    std::fs::create_dir_all(&updates).unwrap();
    let state_path = updates.join("state.json");
    let current = Sha::digest(b"flipped");
    let previous = Sha::digest(b"before");
    let pending = Phase::PendingHealthy(PendingHealthy {
        current,
        previous,
        boots: 0,
        pinned_sequence: 3,
    });
    std::fs::write(&state_path, state::encode(&pending)).unwrap();

    let (_, module) = module_path!().split_once("::").unwrap();
    let child = own_binary_copy_in(root.path())
        .args(["--exact", "--ignored", "--nocapture"])
        .arg(format!("{module}::launched_without_a_key_child"))
        .env("DUCKTAPE_RELEASE", current.to_string())
        .env("DUCKTAPE_UPDATE_STATE", &state_path)
        .env("XDG_DATA_HOME", root.path().join("data"))
        .output()
        .unwrap();
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(child.status.success(), "{out}");
    assert!(out.contains("1 passed"), "the child ran: {out}");

    let settled = state::decode(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(
        settled,
        Phase::Idle(Idle {
            current,
            previous: Some(previous),
            pinned_sequence: 3,
        })
    );
    assert!(
        !updates.join("keys").exists(),
        "no key was pinned on the way"
    );
}

/// A command for this test binary, run from a private copy in `dir`: another
/// `cargo test` sharing the target dir can relink the original mid-run, and
/// from then on `current_exe()` names a deleted file.
fn own_binary_copy_in(dir: &std::path::Path) -> std::process::Command {
    // Linux's /proc/self/exe still opens the running image after the relink
    // unlinked its path; elsewhere the path is all there is.
    let running = if cfg!(target_os = "linux") {
        std::path::PathBuf::from("/proc/self/exe")
    } else {
        std::env::current_exe().unwrap()
    };
    let copy = dir.join("self-test-binary");
    std::fs::copy(running, &copy).unwrap();
    std::process::Command::new(copy)
}

/// The app half of the test above, run only in its child.
#[test]
#[ignore = "run by a_launched_app_without_a_release_key_still_reports_it_rendered"]
fn launched_without_a_key_child() {
    let mut app = Ducktape::initial_state();
    let reading = app
        .update_reading()
        .expect("the launcher env makes an updater with or without a key");
    assert!(matches!(
        reading.phase,
        app_update::Phase::PendingHealthy(_)
    ));

    // the channel is off: neither the wall tick nor Check now starts a fetch
    let now = app.wall_now;
    assert_eq!(app.updater.as_mut().unwrap().tick(now, true), None);
    let _ = app.update(AppMessage::UpdateAction(UpdateAction::CheckNow));
    assert!(!app.update_reading().unwrap().busy, "nothing is fetched");
    let facts = app.update_facts();
    assert_eq!(facts.checked, "never");
    assert_eq!(facts.note, "Updates are off: no release key is pinned.");

    // the first window is the healthy signal all the same
    let _ = app.update(AppMessage::OnboardingOpened(
        crate::shell::WindowKey::unique(),
    ));
    assert_eq!(app.update_facts().state, "idle");
}

#[test]
fn call_presentation_comes_from_the_deployed_guest() {
    let (mut app, _) = Ducktape::boot();
    let _ = app.update(AppMessage::CallEvent(crate::call::CallEvent {
        kind: "presentation".into(),
        stage: "guest-selected-image".into(),
        tiles: vec!["second".into(), "first".into()],
        video_live: true,
        peers: vec![crate::call::CallPeer {
            peer: "first-peer".into(),
            ..Default::default()
        }],
        ..Default::default()
    }));
    assert_eq!(app.huddle_stage, "guest-selected-image");
    assert!(app.call_video_live);
    assert_eq!(app.huddle_tiles, ["second", "first"]);
    assert_eq!(app.call_peers[0].peer, "first-peer");
    // Native self observations cannot choose a different stage or hide the
    // strip. The next guest presentation owns that decision.
    let _ = app.update(AppMessage::CallEvent(crate::call::CallEvent {
        kind: "self".into(),
        sharing: true,
        ..Default::default()
    }));
    assert_eq!(app.huddle_stage, "guest-selected-image");
    let _ = app.update(AppMessage::CallEvent(crate::call::CallEvent {
        kind: "presentation".into(),
        tiles: vec!["replacement".into()],
        peers: vec![crate::call::CallPeer {
            peer: "second-peer".into(),
            ..Default::default()
        }],
        ..Default::default()
    }));
    assert_eq!(app.call_peers.len(), 1);
    assert_eq!(app.call_peers[0].peer, "second-peer");
    // A host runtime trap cannot ask the retired guest to clear its own
    // presentation. Disposal still belongs to the native lifecycle adapter.
    let _ = app.update(AppMessage::CallEvent(crate::call::CallEvent {
        kind: "error".into(),
        message: "guest trapped".into(),
        ..Default::default()
    }));
    assert!(app.call_peers.is_empty());
    assert!(app.huddle_tiles.is_empty());
    assert!(app.huddle_stage.is_empty());
    assert!(!app.call_video_live);
    let _ = app.update(AppMessage::CallEvent(crate::call::CallEvent {
        kind: "presentation".into(),
        stage: "replacement-image".into(),
        video_live: true,
        ..Default::default()
    }));
    let _ = app.update(AppMessage::CallEvent(crate::call::CallEvent {
        kind: "presentation".into(),
        ..Default::default()
    }));
    assert!(app.huddle_stage.is_empty());
    assert!(!app.call_video_live);
}

#[test]
fn call_status_is_the_deployed_views_text() {
    let (mut app, _) = Ducktape::boot();
    let _ = app.update(AppMessage::CallEvent(crate::call::CallEvent {
        kind: "live".into(),
        status: Some("Custom session status".into()),
        ..Default::default()
    }));
    assert_eq!(app.call_status, "Custom session status");
    let _ = app.update(AppMessage::CallEvent(crate::call::CallEvent {
        kind: "presentation".into(),
        ..Default::default()
    }));
    assert_eq!(app.call_status, "Custom session status");
}

/// THE FOUNDING HINT IS A COMMAND (#20). The empty network list is the one
/// place the app says how to found a network; a `<name>` placeholder wrapped
/// inside its own token and read as punctuation beside a sentence period.
#[test]
fn the_founding_command_is_typed_as_shown() {
    let command = crate::shell::FOUNDING_COMMAND;
    assert!(command.starts_with("ducktape node init --name "));
    assert!(
        !command.contains(['<', '>', '.']),
        "{command} carries no placeholder brackets or sentence punctuation"
    );
}

/// BACK OUT OF A JOIN'S WAIT (#18). The blocked wait offers Back to networks;
/// the poll it leaves behind must not open the Live screen over the network
/// list once the member's node answers.
#[test]
fn leaving_a_joins_wait_drops_its_poll() {
    let (mut app, _) = Ducktape::boot();
    let _ = app.update(AppMessage::WorkspaceMaterialized(backend::WorkspaceInit {
        chain_id: "dognet#d2a0ec8f".into(),
        workspace: "/nowhere/dognet#d2a0ec8f".into(),
        rpc: "http://127.0.0.1:1".into(),
    }));
    assert_eq!(app.hub_step, crate::HubStep::Provisioning);
    let waiting = app.provision_progress_generation;

    let _ = app.update(AppMessage::GoNetworks);
    let answered = backend::ProvisionStep {
        index: 5,
        label: "Node API listening · http://127.0.0.1:1".into(),
        state: "done".into(),
        settled: true,
        hint: String::new(),
        command: String::new(),
        ports_held: String::new(),
    };
    let _ = app.update(AppMessage::ProvisionProgressReply(
        waiting,
        Some(Box::new(AppMessage::ProvisionStepped(answered))),
    ));
    assert_eq!(app.hub_step, crate::HubStep::Networks);
}

/// THE READY SCREEN MINTS WHEN ASKED (#9). It minted an invitation the moment
/// it opened, and the node's refusal stayed under "Your network is ready" in
/// red for an invitation nobody had asked for. Nothing is minted until Copy
/// invitation is pressed; a refusal is a sentence beside the button, not the
/// screen's error, and the next press asks again; a minted one is copied, and
/// the node's notes on it (#69 — reachable on this machine only, …) are
/// sentences beside the button too, never the screen's error.
#[tokio::test(flavor = "current_thread")]
async fn the_ready_screen_mints_an_invitation_only_when_asked() {
    use futures::StreamExt as _;
    use gpui_kit::test::TestWindowExt as _;
    let (mut app, _) = Ducktape::boot();
    let _ = app.update(AppMessage::WorkspaceMaterialized(backend::WorkspaceInit {
        chain_id: "nowhere#00000009".into(),
        workspace: "/nowhere/nowhere#00000009".into(),
        rpc: "http://127.0.0.1:1".into(),
    }));
    let waiting = app.provision_progress_generation;
    let answered = backend::ProvisionStep {
        index: 5,
        label: "Node API listening · http://127.0.0.1:1".into(),
        state: "done".into(),
        settled: true,
        hint: String::new(),
        command: String::new(),
        ports_held: String::new(),
    };
    let ready = app.update(AppMessage::ProvisionProgressReply(
        waiting,
        Some(Box::new(AppMessage::ProvisionStepped(answered))),
    ));
    assert_eq!(app.hub_step, crate::HubStep::Live);
    assert!(
        ready.into_stream().next().await.is_none(),
        "nothing is minted on arrival"
    );

    for press in ["first", "second"] {
        let asked = app.update(AppMessage::CopyOnboardingInvite);
        let Some(refused @ AppMessage::OnboardingInviteRefused(_)) =
            asked.into_stream().next().await
        else {
            panic!("the {press} press asks for an invitation");
        };
        let _ = app.update(refused);
        assert!(
            app.invite_refusal
                .starts_with("Your node did not make an invitation: "),
            "{}",
            app.invite_refusal
        );
        assert!(app.onboarding_error.is_empty(), "{}", app.onboarding_error);
        assert_ne!(app.toast, "Invite copied");
        assert_eq!(app.hub_step, crate::HubStep::Live);
    }

    // what the mint could not do rides beside the blob, in the node's words.
    let reachable_here = "this invite is reachable on this machine only";
    let _ = app.update(AppMessage::OnboardingInviteMinted(backend::Invitation {
        blob: "minted-blob".into(),
        notes: vec![reachable_here.into()],
    }));
    assert_eq!(app.invite_link, "minted-blob");
    assert_eq!(app.invite_notes, [reachable_here]);
    assert!(app.invite_refusal.is_empty());
    assert!(app.onboarding_error.is_empty(), "{}", app.onboarding_error);
    assert_eq!(app.toast, "Invite copied");

    // … and the ready screen draws that toast (#75): it was the workspace's
    // alone, so the copy confirmed nothing where the button is.
    let (mut native, window, view) = onboarding_window(app);
    window
        .update(&mut native, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("toast-dismiss").visible());

            // the next network's ready screen starts with no notes of its own.
            view.update(cx, |view, cx| {
                view.test_dispatch(
                    AppMessage::WorkspaceMaterialized(backend::WorkspaceInit {
                        chain_id: "nowhere#0000000a".into(),
                        workspace: "/nowhere/nowhere#0000000a".into(),
                        rpc: "http://127.0.0.1:1".into(),
                    }),
                    cx,
                )
            });
            assert!(view.read(cx).test_state(cx).invite_notes.is_empty());
        })
        .unwrap();
}

/// The join screens' window, holding `state`.
fn onboarding_window(
    state: Ducktape,
) -> (
    gpui_kit::HeadlessAppContext,
    gpui_kit::AnyWindowHandle,
    gpui_kit::Entity<crate::shell::DesktopWindow>,
) {
    use gpui_kit::AppContext as _;
    let mut native = crate::frame_probe::headless_context();
    let mut view = None;
    let window = native
        .open_window(
            gpui_kit::size(gpui_kit::px(480.), gpui_kit::px(680.)),
            |window, cx| {
                let desktop = crate::shell::test_window(
                    state,
                    crate::shell::WindowKind::Onboarding,
                    window,
                    cx,
                );
                view = Some(desktop.clone());
                cx.new(|cx| gpui_kit::component::Root::new(desktop, window, cx))
            },
        )
        .expect("the join screens open");
    (native, window.into(), view.expect("the window's view"))
}

/// COPY COMMAND SAYS SO (#75). The waiting step's Copy command put the node's
/// command on the clipboard and drew nothing: its toast was drawn only by the
/// workspace. The press confirms itself on the step.
#[test]
fn the_waiting_steps_copy_command_confirms_itself() {
    use gpui_kit::test::TestWindowExt as _;
    let mut app = Ducktape::initial_state();
    app.hub_step = HubStep::Provisioning;
    app.provision_steps = vec![backend::ProvisionStep {
        index: 5,
        label: "Node API listening".into(),
        state: "waiting".into(),
        settled: false,
        hint: String::new(),
        command: "ducktape node run".into(),
        ports_held: String::new(),
    }];
    let (mut native, window, view) = onboarding_window(app);
    window
        .update(&mut native, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("toast-dismiss").is_none());
            window.click("copy-node-command", cx);
            window.render_frame(cx);
            assert_eq!(view.read(cx).test_state(cx).toast, "Command copied");
            assert!(window.find("toast-dismiss").visible());
            // in the body's corner, as in the workspace's content box: not
            // straddling the hairline of the 36px footer band below it.
            let footer_top = window.viewport_size().height - gpui_kit::px(36.);
            let toast = window.find("toast-dismiss").bounds();
            assert!(toast.bottom() <= footer_top, "{toast:?} crosses the footer");
        })
        .unwrap();
}

/// THE WAIT'S COMMAND IS NOT ITS LABEL (#87). The node's two-line command was
/// the step's label, set in the UI face beside the state word: it wrapped to
/// some 25 lines, so the state, Copy command and the hint saying why the step
/// is blocked fell below the fold of the join window, and Back to networks
/// with them. The label stays a label; the command is its own block under it.
#[test]
fn the_waiting_steps_command_leaves_the_step_in_view() {
    use gpui_kit::test::TestWindowExt as _;
    let key = "ab".repeat(32);
    for release_key in [Some(key.as_str()), None] {
        let mut app = Ducktape::initial_state();
        app.hub_step = HubStep::Provisioning;
        // long past its patience: blocked, with the longest hint.
        app.provision_steps = vec![backend::node_wait_step(
            "/home/member/.ducktape/dognet#d2a0ec8f",
            release_key,
            u32::MAX,
            "",
        )];
        let (mut native, window, _) = onboarding_window(app);
        window
            .update(&mut native, |_, window, cx| {
                window.render_frame(cx);
                let footer_top = window.viewport_size().height - gpui_kit::px(36.);
                for id in ["copy-node-command", "provision-back"] {
                    let bounds = window.find(id).bounds();
                    assert!(
                        bounds.bottom() <= footer_top,
                        "{id} {bounds:?} is below the fold"
                    );
                }
            })
            .unwrap();
    }
}

/// AN OFFLINE NETWORK SAYS SO FIRST (#19). A saved row reading `offline` still
/// opened — by design — and the open then ran the whole wallet ceremony:
/// password, 24 words, the confirm. Only the account lookup after it failed,
/// in the HTTP client's words, under a Confirm heading whose instructions were
/// gone and with no way back. The open asks the node before any wallet screen,
/// and a node that does not answer is the offline step, with a retry and the
/// way back; a lookup that fails after a ceremony wrote the key says the
/// wallet is saved, on that step, never on the spent screen.
#[tokio::test(flavor = "current_thread")]
async fn an_offline_network_says_so_before_any_wallet_step() {
    use futures::StreamExt as _;
    /// Run a task's messages back through the app, as the runtime does, and
    /// note every step the launch window passed through.
    async fn settle(app: &mut Ducktape, task: view_wire::Task<AppMessage>) -> Vec<HubStep> {
        let mut steps = Vec::new();
        let mut pending = vec![task];
        while let Some(task) = pending.pop() {
            let mut messages = task.into_stream();
            while let Some(message) = messages.next().await {
                pending.push(app.update(message));
                steps.push(app.hub_step);
            }
        }
        steps
    }
    let wallet_screens = [
        HubStep::Password,
        HubStep::Phrase,
        HubStep::Confirm,
        HubStep::Wallets,
        HubStep::Restore,
        HubStep::Account,
    ];
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        format!("http://{}", listener.local_addr().expect("its address"))
    };
    let unreachable = format!("Can't reach this network's node at {dead}.");
    let (mut app, _) = Ducktape::boot();
    app.hub_step = HubStep::Networks;
    app.hub_networks = vec![backend::HubNetwork {
        id: "dognet#d2a0ec8f".into(),
        chain_id: "dognet#d2a0ec8f".into(),
        name: "dognet".into(),
        endpoint: dead.clone(),
        endpoint_override: String::new(),
        kind: "local".into(),
        last_used: 0,
        probed: true,
        live: false,
        height: -1,
        contract: 0,
        another_network: false,
        phase: String::new(),
        behind_by: -1,
        netstack_failure: String::new(),
    }];
    app.hub_selected = "dognet#d2a0ec8f".into();
    assert_eq!(
        backend::network_row_label(&app.hub_networks[0]),
        "dognet · offline"
    );

    let open = app.update(AppMessage::OpenNetworkSubmit);
    assert_eq!(
        app.mutation_phase,
        MutationPhase::Onboarding,
        "a dead row still opens"
    );
    let steps = settle(&mut app, open).await;
    assert!(
        !steps.iter().any(|step| wallet_screens.contains(step)),
        "{steps:?}"
    );
    assert_eq!(app.hub_step, HubStep::Offline);
    assert_eq!(app.onboarding_error, unreachable);
    assert_eq!(app.mutation_phase, MutationPhase::Idle);

    // Retry asks the same node again; still down, still offline.
    let retry = app.update(AppMessage::RetryNetwork);
    assert_eq!(app.mutation_phase, MutationPhase::Onboarding);
    let steps = settle(&mut app, retry).await;
    assert!(
        !steps.iter().any(|step| wallet_screens.contains(step)),
        "{steps:?}"
    );
    assert_eq!(app.hub_step, HubStep::Offline);
    assert_eq!(app.onboarding_error, unreachable);

    let _ = app.update(AppMessage::GoNetworks);
    assert_eq!(app.hub_step, HubStep::Networks);
    assert!(app.onboarding_error.is_empty());

    // A CEREMONY THAT FINISHED, against a node that is gone by the lookup: the
    // confirm sealed the key and seated it before the account was asked for
    // (`confirm_recovery_phrase`), which is what these two lines stand in for —
    // the seal itself writes into the real home, which is not a test's to touch.
    app.rpc = dead.clone();
    app.hub_step = HubStep::Confirm;
    app.mutation_phase = MutationPhase::Onboarding;
    backend::set_local_user_key(Some(vec![7; 32])).await;
    let confirmed = app.update(AppMessage::PhraseConfirmed("07".repeat(32)));
    let steps = settle(&mut app, confirmed).await;
    backend::set_local_user_key(None).await;
    assert!(!steps.contains(&HubStep::Account), "{steps:?}");
    assert_eq!(
        app.hub_step,
        HubStep::Offline,
        "the spent Confirm screen is left"
    );
    assert_eq!(
        app.onboarding_error,
        format!("Your wallet is saved. {unreachable}")
    );
    assert_eq!(
        app.signer_key,
        "07".repeat(32),
        "the saved wallet is still the seat"
    );
    assert_eq!(app.mutation_phase, MutationPhase::Idle);
}

/// CREATE A WALLET ON WELCOME BACK MAKES A WALLET (#24). The button sent the
/// skip, so a keystore holding any wallet — one whose password is gone, say —
/// could never reach the ceremony again: the workspace opened unsigned. It
/// opens the same password step an empty keystore lands on, and the skip is
/// its own button.
#[test]
fn create_a_wallet_on_welcome_back_opens_the_wallet_ceremony() {
    use futures::StreamExt as _;
    let (mut app, _) = Ducktape::boot();
    app.rpc = "http://127.0.0.1:1".into();
    let _ = app.update(AppMessage::WalletsLoaded(backend::WalletList {
        wallets: vec![backend::WalletInfo {
            name: "zk-dev".into(),
            pubkey: "07".repeat(32),
            state: "encrypted".into(),
            active: true,
        }],
        error: String::new(),
        keystore: true,
        offline: false,
    }));
    assert_eq!(app.hub_step, HubStep::Wallets);

    let create = app.update(AppMessage::GoCreateWallet);
    let queued = futures::executor::block_on(create.into_stream().collect::<Vec<_>>());
    assert!(queued.is_empty(), "creating never enters the workspace");
    assert_eq!(app.hub_step, HubStep::Password);

    let _ = app.update(AppMessage::GoLogin);
    assert_eq!(app.hub_step, HubStep::Wallets, "Back returns to the list");
    let skip = app.update(AppMessage::LoginSkip);
    let queued = futures::executor::block_on(skip.into_stream().collect::<Vec<_>>());
    assert!(
        matches!(queued.as_slice(), [AppMessage::NetworkEntered]),
        "the skip still enters the workspace"
    );
}

/// CANCEL ON AN IDLE ACCOUNT SCREEN GOES BACK (#25). Cancel only stopped a
/// ceremony, so with none running the press did nothing — and the workspace
/// the banner's Sign in had closed stayed closed. A ceremony in flight is
/// still what Cancel stops; an idle screen goes back where it was opened
/// from: the workspace, or the wallet step the key was just opened on.
#[test]
fn cancel_on_an_idle_account_screen_goes_back() {
    use futures::StreamExt as _;
    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    app.connected_rpc = "http://127.0.0.1:1".into();
    let _ = app.update(AppMessage::OpenAccountWelcome);
    let _ = app.update(AppMessage::WelcomeReopened(
        crate::shell::WindowKey::unique(),
    ));
    assert_eq!(app.hub_step, HubStep::Account);

    app.mutation_phase = MutationPhase::Onboarding;
    app.ceremony_phase = "working".into();
    let cancel = app.update(AppMessage::WelcomeCancel);
    let queued = futures::executor::block_on(cancel.into_stream().collect::<Vec<_>>());
    assert!(
        queued.is_empty(),
        "a ceremony in flight is stopped, not left"
    );
    assert_eq!(app.hub_step, HubStep::Account);
    assert!(app.ceremony_phase.is_empty());
    assert_eq!(app.mutation_phase, MutationPhase::Idle);

    let cancel = app.update(AppMessage::WelcomeCancel);
    let queued = futures::executor::block_on(cancel.into_stream().collect::<Vec<_>>());
    assert!(
        matches!(queued.as_slice(), [AppMessage::NetworkEntered]),
        "opened from the workspace, Cancel returns to it"
    );

    // opened by a wallet step, before any workspace: back to that step, read
    // again — a wallet minted on the way is not in the list loaded before it.
    let (mut app, _) = Ducktape::boot();
    app.rpc = "http://127.0.0.1:1".into();
    app.hub_step = HubStep::Wallets;
    app.password = "a password".into();
    let _ = app.update(AppMessage::AccountProbed(backend::AccountData {
        generation: 0,
        exists: false,
        number: String::new(),
        name: String::new(),
        bio: String::new(),
    }));
    assert_eq!(app.hub_step, HubStep::Account);
    let asked = app.wallets_load_generation;
    let _ = app.update(AppMessage::WelcomeCancel);
    assert_ne!(
        app.wallets_load_generation, asked,
        "the wallet step is asked"
    );
    assert!(app.password.is_empty());
    let _ = app.update(AppMessage::WalletsLoadReply(
        app.wallets_load_generation,
        Box::new(AppMessage::WalletsLoaded(backend::WalletList {
            wallets: vec![backend::WalletInfo {
                name: "zk-dev".into(),
                pubkey: "07".repeat(32),
                state: "encrypted".into(),
                active: true,
            }],
            error: String::new(),
            keystore: true,
            offline: false,
        })),
    ));
    assert_eq!(app.hub_step, HubStep::Wallets);
}

/// ENTERING WITHOUT A WALLET DROPS AN UNLOCKED KEY (#38). A key unlocked on
/// the wallet step stayed seated after the account screen was left, so
/// "Continue without a wallet" on the list it went back to opened a signed
/// workspace. An entry with no password is unsigned, whichever way it came
/// in; picking the wallet still enters signed.
#[test]
fn entering_without_a_wallet_drops_an_unlocked_key() {
    use futures::StreamExt as _;
    let key = "07".repeat(32);
    let listed = || {
        AppMessage::WalletsLoaded(backend::WalletList {
            wallets: vec![backend::WalletInfo {
                name: "zk-dev".into(),
                pubkey: "07".repeat(32),
                state: "encrypted".into(),
                active: true,
            }],
            error: String::new(),
            keystore: true,
            offline: false,
        })
    };
    let account = |exists| {
        AppMessage::AccountProbed(backend::AccountData {
            generation: 0,
            exists,
            number: String::new(),
            name: String::new(),
            bio: String::new(),
        })
    };
    let (mut app, _) = Ducktape::boot();
    app.rpc = "http://127.0.0.1:1".into();
    let _ = app.update(listed());
    let _ = app.update(AppMessage::UnlockSubmit("a password".into()));
    let _ = app.update(AppMessage::KeyUnlocked(key.clone()));
    let _ = app.update(account(false));
    assert_eq!(app.hub_step, HubStep::Account);
    let _ = app.update(AppMessage::WelcomeCancel);
    let _ = app.update(AppMessage::WalletsLoadReply(
        app.wallets_load_generation,
        Box::new(listed()),
    ));
    assert_eq!(app.hub_step, HubStep::Wallets);
    let skip = app.update(AppMessage::LoginSkip);
    let queued = futures::executor::block_on(skip.into_stream().collect::<Vec<_>>());
    assert!(matches!(queued.as_slice(), [AppMessage::NetworkEntered]));
    let _ = app.update(AppMessage::NetworkEntered);
    assert_eq!(
        backend::rail_identity(false, "", "", &app.signer_key).1,
        "No signing key",
        "the skip enters unsigned"
    );

    let (mut app, _) = Ducktape::boot();
    app.rpc = "http://127.0.0.1:1".into();
    let _ = app.update(listed());
    let _ = app.update(AppMessage::UnlockSubmit("a password".into()));
    let _ = app.update(AppMessage::KeyUnlocked(key.clone()));
    let found = app.update(account(true));
    let queued = futures::executor::block_on(found.into_stream().collect::<Vec<_>>());
    assert!(matches!(queued.as_slice(), [AppMessage::NetworkEntered]));
    let _ = app.update(AppMessage::NetworkEntered);
    assert_eq!(app.signer_key, key, "the picked wallet enters signed");
}
