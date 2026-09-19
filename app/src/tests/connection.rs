use super::*;

#[test]
fn connection_progress_keeps_the_task_alive_and_retires_previous_attempt_errors() {
    let (mut app, _) = Ducktape::boot();
    app.rpc = "http://127.0.0.1:38259".into();
    app.hub_step = HubStep::Wallets;
    let _ = app.update(AppMessage::NetworkEntered);
    let first_request = app.connection_generation;
    let first_connect = app.connect_generation;
    let _ = app.update(AppMessage::ConnectionReply(
        first_request,
        Some(Box::new(AppMessage::ConnectionProgress(
            first_connect,
            "Loading chat and workspace…",
        ))),
    ));
    assert!(
        app.connection_task.is_some(),
        "progress must not abort its own connection"
    );

    let _ = app.update(AppMessage::ConnectionReply(
        first_request,
        Some(Box::new(AppMessage::ConnectFailed(
            backend::HydrationError {
                generation: first_connect,
                message: "chat view failed".into(),
            },
        ))),
    ));
    assert_eq!(
        app.connection_progress,
        "Retrying automatically in 1s · retry 1"
    );
    assert_eq!(app.onboarding_error, "chat view failed");
    let retry_request = app.connection_generation;
    let retry_connect = app.connect_generation;
    assert_ne!(retry_request, first_request);
    let _ = app.update(AppMessage::ConnectionReply(first_request, None));
    assert!(
        app.connection_task.is_some(),
        "the previous stream cannot cancel the retry"
    );
    let _ = app.update(AppMessage::ConnectionProgress(
        first_connect,
        "stale progress",
    ));
    assert_eq!(app.onboarding_error, "chat view failed");
    assert_ne!(app.connection_progress, "stale progress");

    let _ = app.update(AppMessage::ConnectionReply(
        retry_request,
        Some(Box::new(AppMessage::ConnectionProgress(
            retry_connect,
            "Preparing workspace screens…",
        ))),
    ));
    assert_eq!(app.connection_progress, "Preparing workspace screens…");
    assert!(app.onboarding_error.is_empty());
    assert!(app.connection_task.is_some());
    let _ = app.update(AppMessage::ConnectionReply(retry_request, None));
    assert!(app.connection_task.is_none());
}

#[test]
fn returning_to_networks_cancels_the_pending_connection_and_ignores_its_success() {
    let (mut app, _) = Ducktape::boot();
    app.rpc = "http://127.0.0.1:38259".into();
    app.hub_step = HubStep::Wallets;
    let _ = app.update(AppMessage::NetworkEntered);
    let request = app.connection_generation;
    let generation = app.connect_generation;
    assert!(app.connection_task.is_some());
    let _ = app.update(AppMessage::GoNetworks);
    assert_eq!(app.console_entry, ConsoleEntry::Idle);
    assert!(app.connection_task.is_none());
    assert!(app.connection_progress.is_empty());
    let mut late = workspace("late-channel");
    late.generation = generation;
    let _ = app.update(AppMessage::ConnectionReply(
        request,
        Some(Box::new(AppMessage::WorkspaceConnected(late.clone()))),
    ));
    let _ = app.update(AppMessage::WorkspaceConnected(late));
    assert!(!app.connected);
    assert_ne!(app.active_channel, "late-channel");
    assert_eq!(app.hub_step, HubStep::Networks);
}

#[tokio::test]
async fn a_connection_announces_its_work_before_returning_a_failure() {
    use futures::StreamExt as _;
    let mut task = backend::connect("not a node address".into(), 0, 7).into_stream();
    assert!(matches!(
        task.next().await,
        Some(AppMessage::ConnectionProgress(
            7,
            "Loading chat and workspace…"
        ))
    ));
    assert!(matches!(
        task.next().await,
        Some(AppMessage::ConnectFailed(backend::HydrationError {
            generation: 7,
            ..
        }))
    ));
    assert!(task.next().await.is_none());
}

/// A LOAD FAILED; THE CONNECTION SAID NOTHING.
///
/// The generic failed arm wrote `status = "Offline"` for one slow load, over a
/// live socket — and `connected` stays true, so nothing reconnects and nothing
/// corrects it: the sidebar dot goes red and the pill reads Offline until the
/// next block's `live_updated` overwrites the status, up to 3s on a quiet chain.
#[test]
fn a_single_failed_load_does_not_report_the_connection_offline() {
    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    app.loading = true;
    app.status = "Live".into();

    let _ = app.update(AppMessage::ChatLoadFailed(backend::HydrationError {
        generation: app.chat_generation,
        message: "the channel did not load".into(),
    }));

    assert_eq!(
        app.status, "Live",
        "the connection's word belongs to the connection's own handlers"
    );
    assert_eq!(app.error, "the channel did not load");
    assert!(!app.loading, "and the pane is released either way");
}

/// AN ERROR MUST NOT ASSERT A DIAGNOSIS IT HAS NOT MADE. `connect` discarded
/// the real cause with `map_err(|_| …)` and said "Could not connect. Check the
/// endpoint and node." — the one thing the reader can act on, and wrong
/// whenever the node is answering fine and the failure is a timeout, an
/// unreadable reply or a broken signer. Measured while debugging this screen:
/// the node served `/v1/status` in under a millisecond and the app still said
/// to go check it.
///
/// Pinned as a source shape because the failure is an async RPC round trip with
/// no seam to fake here; `user_error` itself is covered by its own tests.
#[test]
fn connect_reports_the_cause_instead_of_guessing_at_it() {
    const LIVE: &str = include_str!("../backend/live.rs");
    let connect = LIVE
        .split("pub fn connect(")
        .nth(1)
        .expect("connect is declared")
        .split("\npub ")
        .next()
        .expect("connect body");

    assert!(
        connect.contains("user_error(cause.to_string())"),
        "connect must route its cause through the translator the rest of the app uses"
    );
    assert!(
        !connect.contains("map_err(|_|"),
        "throwing the cause away is what made this error a guess"
    );
    // NOT asserted: that the old sentence is absent from the function. The
    // comment above the fix quotes it to explain what was wrong, and a sweep
    // over source text cannot tell a message from the prose about it — the
    // check would fail on its own documentation.
}

/// THE CONSOLE HEALS ITSELF FROM A CONNECT FAILURE. The steady-state path has
/// always retried forever (`live_resync_failed`), so the app recovered from
/// every interruption except the one that gets it running: `on failed` set
/// Offline and stopped, leaving the console dead against a node answering
/// `/v1/status` in under a millisecond, with no way back but the network
/// picker. Issue #1018 makes that failure ordinary — a `/v1/query` can block
/// until the node writes its next checkpoint, outlasting the client's 30s
/// timeout.
#[test]
fn a_failed_connect_retries_instead_of_giving_up() {
    let (mut app, _) = Ducktape::boot();
    app.connected = true;
    let before = app.connect_generation;

    let fail = |generation: i64| {
        AppMessage::ConnectFailed(backend::HydrationError {
            generation,
            message: "error sending request".into(),
        })
    };
    let retry = app.update(fail(app.connect_generation));
    {
        use futures::{FutureExt as _, StreamExt as _};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let mut retry = retry.into_stream();
        assert!(
            retry.next().now_or_never().is_none(),
            "failure schedules a backoff task, rather than an empty completed task"
        );
    }
    assert_eq!(
        app.hydration_retry_attempt, 1,
        "the first failure is attempt 1"
    );
    assert_eq!(app.status, "Offline", "and it is offline while it retries");
    assert!(
        app.connect_generation > before,
        "each attempt owns a generation, so an abandoned one cannot answer"
    );

    // The counter CLIMBS — that is what feeds the backoff. A reset here would
    // retry at 1s forever against a genuinely dead endpoint.
    let _ = app.update(fail(app.connect_generation));
    let _ = app.update(fail(app.connect_generation));
    assert_eq!(app.hydration_retry_attempt, 3);

    // A CONNECT IS NOT GUARDED ON `hydration_generation`, AND THIS IS WHY.
    // Thirty-seven handlers bump that counter for reasons of their own —
    // `choose_channel` is one of them — and a connect is in flight for seconds,
    // or for up to 30s while the node sits in issue #1018's checkpoint stall.
    // Guarded on the shared counter, one click on a channel mid-connect drops
    // the successful reply; because it SUCCEEDED no failure arm fires and
    // nothing retries, so the console sits Offline forever. Strictly worse than
    // the defect this PR fixes.
    let (mut wired, _) = Ducktape::boot();
    wired.connected_rpc = "http://127.0.0.1:38259".into();
    let connect_gen = wired.connect_generation;
    let shared_before = wired.hydration_generation;
    let _ = wired.update(AppMessage::ChooseChannel("general".into()));
    assert!(
        wired.hydration_generation > shared_before,
        "an ordinary channel click bumps the SHARED counter"
    );
    assert_eq!(
        wired.connect_generation, connect_gen,
        "and leaves the connect's own alone — only the three routes that start a connect may touch it"
    );

    // AND A FAILURE FROM AN ABANDONED CHAIN IS DROPPED UNREAD. Without this,
    // two chains retry forever and each one's generation bump can reject the
    // other's success — measured live as two interleaved retry series 5.2s and
    // 10.8s apart, summing to one 16s cap.
    let stale = app.connect_generation - 1;
    let attempts = app.hydration_retry_attempt;
    let _ = app.update(fail(stale));
    assert_eq!(
        app.hydration_retry_attempt, attempts,
        "an abandoned chain must not start a second retry loop"
    );

    let mut stale_reply = workspace("abandoned-channel");
    stale_reply.generation = connect_gen - 1;
    let rpc = wired.connected_rpc.clone();
    let _ = wired.update(AppMessage::WorkspaceConnected(stale_reply));
    assert_eq!(
        wired.connected_rpc, rpc,
        "an abandoned success cannot replace the endpoint"
    );

    let mut landed = workspace("general");
    landed.generation = connect_gen;
    let rpc = landed.rpc.clone();
    wired.hydration_retry_attempt = 3;
    wired.onboarding_error = "another wallet's error".into();
    let _ = wired.update(AppMessage::WorkspaceConnected(landed));
    assert_eq!(wired.onboarding_error, "another wallet's error");
    assert_eq!(wired.connected_rpc, rpc);
    assert_eq!(
        wired.active_channel, "general",
        "the matching connect succeeds despite unrelated hydration generation changes"
    );
    assert_eq!(
        wired.hydration_retry_attempt, 0,
        "success clears the backoff"
    );
}

/// THE OPEN WAIT HEARS THE NODE (#27). A syncing node holds the workspace load
/// for minutes; the status push the console holds starts when the wait does, so
/// the wait can name the phase — and it is the same stream the console keeps,
/// not a second one. A phase left from another network is not this one's.
#[test]
fn the_open_wait_holds_the_status_push_the_console_keeps() {
    let keys = |app: &Ducktape| {
        app.subscriptions()
            .into_recipes()
            .into_iter()
            .map(|recipe| recipe.key)
            .collect::<Vec<_>>()
    };
    let (mut app, _) = Ducktape::boot();
    app.rpc = "http://127.0.0.1:38259".into();
    app.hub_step = HubStep::Wallets;
    app.node_phase = "syncing".into();
    let idle = keys(&app);

    let _ = app.update(AppMessage::NetworkEntered);
    assert!(app.node_phase.is_empty());
    let entering = keys(&app);
    let added: Vec<_> = entering.iter().filter(|key| !idle.contains(key)).collect();
    assert_eq!(added.len(), 1, "the wait holds the status push alone");
    let status = *added[0];

    app.connected = true;
    assert!(keys(&app).contains(&status));

    app.connected = false;
    let _ = app.update(AppMessage::GoNetworks);
    assert!(!keys(&app).contains(&status));
}

/// Launch the app on `link` and enter a network, delivering the node's facts
/// and the console's first live catch-up in the given order, each reply run
/// back through the app as the runtime does.
fn launched_on(link: &str, facts_first: bool) -> Ducktape {
    use futures::{FutureExt as _, StreamExt as _};
    fn settle(app: &mut Ducktape, task: view_wire::Task<AppMessage>) {
        let mut pending = vec![task.into_stream()];
        while let Some(mut messages) = pending.pop() {
            while let Some(Some(message)) = messages.next().now_or_never() {
                pending.push(app.update(message).into_stream());
            }
        }
    }
    let (mut app, _) = Ducktape::boot();
    app.startup_duck_link = link.into();
    app.rpc = "http://127.0.0.1:38259".into();
    app.hub_step = HubStep::Wallets;
    let _ = app.update(AppMessage::NetworkEntered);
    let mut landed = workspace("general");
    landed.generation = app.connect_generation;
    let _ = app.update(AppMessage::WorkspaceConnected(landed));
    let facts = |app: &mut Ducktape| {
        let task = app.update(AppMessage::NodeFactsLoaded(backend::NodeFacts {
            chain_id: "dognet#040b41aa".into(),
            ..backend::NodeFacts::default()
        }));
        settle(app, task);
    };
    if facts_first {
        facts(&mut app);
    }
    let _ = app.update(AppMessage::LiveUpdated(backend::LiveUpdate {
        kind: LiveKind::Ready,
        status: "Live".into(),
        height: -1,
        ..backend::LiveUpdate::default()
    }));
    let caught_up = app.update(AppMessage::LiveResynced(live_refresh(
        app.hydration_generation,
        "general",
    )));
    settle(&mut app, caught_up);
    if !facts_first {
        facts(&mut app);
    }
    app
}

/// A LAUNCH LINK THE APP CANNOT OPEN SAYS SO IN THE CONSOLE (#56). The same
/// address clicked in Chat draws the red banner; handed to the app at launch
/// it was opened while the console was still catching up, and the catch-up's
/// landing cleared the banner before anyone saw it.
#[test]
fn a_launch_link_the_app_cannot_open_says_so_in_the_console() {
    for facts_first in [true, false] {
        let mut app = launched_on(
            "duck://files/shared/no-such-path-rehearsal-open/missing.txt",
            facts_first,
        );
        assert_eq!(
            app.error,
            crate::backend::OLD_FORM,
            "the old form, facts first: {facts_first}"
        );
        assert!(app.startup_duck_link.is_empty(), "opened once");
        let _ = app.update(AppMessage::DismissError);
        assert!(app.error.is_empty());
    }
}

/// The old form is not read at launch: a launch link comes from no view.
#[test]
fn a_launch_link_in_the_old_form_is_refused_whatever_it_names() {
    for link in [
        "duck://page/pg-1",
        "duck://channel/general#42",
        "duck://forge/ducks/core/58",
        "duck://account/7",
    ] {
        let app = launched_on(link, true);
        assert_eq!(app.error, crate::backend::OLD_FORM, "{link}");
    }
}

/// And a well-formed one still lands where it points.
#[test]
fn a_well_formed_launch_link_still_opens_files() {
    let link = "duck://dognet-040b41aa/files/shared/no-such-path-rehearsal-open/missing.txt";
    let app = launched_on(link, true);
    assert_eq!(app.shell_tab, ShellTab::View("files"));
    assert_eq!(app.fs_route, link, "the epoch-10 address reaches the view");
    assert!(app.error.is_empty());
}
