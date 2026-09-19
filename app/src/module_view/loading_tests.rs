//! A view loading off a node says what it is doing, a failure is named and
//! offers Retry, and a seat no layer draws still runs (#110).
use super::tests::{connection_turn, deployment, fresh, hold_status, staged};
use super::*;
use crate::backend::view_source::tests::{FakeDeployment, fake_node, node, status_naming};

/// Seats `module` part-way through a fetch — or failed, named — for a test
/// that draws its tab. [`unseat`] gives the seat back.
pub(crate) fn seat_standin(module: &'static str, failed: bool) {
    let seat = mounted(module);
    let mut seat = seat.lock().expect("module view lock");
    seat.start(None);
    seat.in_flight = false;
    seat.slot = match failed {
        false => Slot::Fetching {
            received: 1_200_000,
            total: Some(3_400_000),
        },
        true => Slot::Failed(Failure::Unreachable(
            "fetch view artifact: connection refused".into(),
        )),
    };
}

pub(crate) fn unseat(module: &str) {
    registry().lock().expect("module views").remove(module);
}

/// The stages a load showed, in order, off the lines the loader taps.
fn stages(tap: &std::sync::mpsc::Receiver<String>, module: &str) -> Vec<String> {
    let prefix = format!("view_stage module={module} ");
    let mut seen: Vec<String> = Vec::new();
    for line in tap.try_iter() {
        let Some(words) = line.strip_prefix(&prefix) else {
            continue;
        };
        let stage = words.split(' ').next().unwrap_or_default().to_owned();
        if seen.last() != Some(&stage) {
            seen.push(stage);
        }
    }
    seen
}

#[test]
fn a_stage_shows_only_over_a_load_and_only_for_the_load_the_seat_waits_for() {
    let seat = Mounted::seat();
    let mut seat = seat.lock().unwrap();
    assert_eq!(stage_words(&seat.slot), "Loading the view…");
    let generation = seat.start(None);
    seat.show(
        generation,
        Slot::Fetching {
            received: 1_200_000,
            total: Some(3_400_000),
        },
    );
    assert_eq!(
        stage_words(&seat.slot),
        "Fetching the view — 1.2 MB of 3.4 MB"
    );
    seat.show(
        generation,
        Slot::Fetching {
            received: 2_500,
            total: None,
        },
    );
    assert_eq!(stage_words(&seat.slot), "Fetching the view — 3 KB");
    seat.show(generation, Slot::Verifying);
    assert!(matches!(seat.slot, Slot::Verifying));
    seat.show(generation, Slot::Compiling);
    assert!(matches!(seat.slot, Slot::Compiling));
    // a load the seat no longer waits for says nothing
    seat.show(generation - 1, Slot::Verifying);
    assert!(matches!(seat.slot, Slot::Compiling));
    // what a landed load left — the network's "none", a failure — stays up
    // until the next load lands
    for landed in [Slot::Empty, Slot::Failed(Failure::NotListed("gone".into()))] {
        seat.slot = landed;
        let generation = seat.start(None);
        seat.show(
            generation,
            Slot::Fetching {
                received: 0,
                total: None,
            },
        );
        assert!(!seat.slot.loading());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_load_off_the_node_fetches_verifies_and_compiles_before_it_seats() {
    let _turn = connection_turn().await;
    let Some(view) = staged("governance") else {
        return;
    };
    let artifact = deployment(&std::fs::read(view).unwrap(), "a.svg");
    let client = fake_node(FakeDeployment::serving("stages", &artifact)).await;
    let tap = canary::tap();
    let seat = Mounted::seat();
    let generation = seat.lock().unwrap().start(None);
    let asked_of = Connection {
        client: Some(client),
        rev: connection().lock().unwrap().rev,
    };
    let loading = seat.clone();
    tokio::task::spawn_blocking(move || {
        spawn_load("stages", &loading, generation, asked_of).join()
    })
    .await
    .unwrap()
    .unwrap();
    let shown = stages(&tap, "stages");
    // the body may land in one read: every byte in at once is verifying
    let fetched = shown.first().is_some_and(|stage| stage == "Fetching");
    assert_eq!(
        &shown[usize::from(fetched)..],
        ["Verifying", "Compiling"],
        "{shown:?}"
    );
    assert!(matches!(seat.lock().unwrap().slot, Slot::Ready(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_way_a_load_fails_is_named_and_offers_retry() {
    let _turn = connection_turn().await;
    let failed = |client: ducktape_rpc::Client| {
        tokio::task::spawn_blocking(move || {
            let seat = Mounted::seat();
            let generation = seat.lock().unwrap().start(None);
            match Guest::load("named", Some(&client), generation, &seat) {
                Err(unloaded) => unloaded.failure,
                Ok(_) => panic!("the load did not fail"),
            }
        })
    };
    let bytes_no_view_runs = deployment(b"not a component", "a.svg");
    let nothing_listens = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        ducktape_rpc::Client::new(&origin).unwrap()
    };
    let lists_nothing = node(
        serde_json::json!({"module_status": {"modules": []}}),
        None,
        None,
    )
    .await;
    let admitted_only = node(
        serde_json::json!({"module_status": {"modules": [
            {"module_id": "named", "kind": "module", "active_code_hash": [],
             "pending": null, "history": []}
        ]}}),
        None,
        None,
    )
    .await;
    let holds_nothing = node(
        status_naming("named", &bytes_no_view_runs.hash()),
        None,
        None,
    )
    .await;
    // the node answers another artifact's bytes under the hash it names
    let answers_other_bytes = node(
        status_naming("named", &bytes_no_view_runs.hash()),
        Some(deployment(b"other bytes", "b.svg")),
        None,
    )
    .await;
    let serves_no_view = fake_node(FakeDeployment::serving("named", &bytes_no_view_runs)).await;
    let named = [
        failed(nothing_listens).await.unwrap(),
        failed(holds_nothing).await.unwrap(),
        failed(answers_other_bytes).await.unwrap(),
        failed(lists_nothing).await.unwrap(),
        failed(admitted_only).await.unwrap(),
        failed(serves_no_view).await.unwrap(),
    ];
    assert!(
        matches!(
            &named,
            [
                Failure::Unreachable(_),
                Failure::Unreachable(_),
                Failure::HashMismatch(_),
                Failure::NotListed(_),
                Failure::NotActivated(_),
                Failure::Refused(_),
            ]
        ),
        "{named:?}"
    );
    // a view that ran out of fuel or trapped is named the same way, and
    // every tab says the name above the reason, and offers Retry
    let stopped = Failure::Trapped("all fuel consumed by WebAssembly".into());
    for failure in named.iter().chain([&stopped]) {
        let standin = Standin::from(failure);
        assert_eq!(standin.title, Some(failure.title()));
        assert_eq!(standin.words, failure.to_string());
        assert!(standin.retry && !standin.loading, "{failure:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_asks_again_for_a_failed_view_and_reseats_a_stopped_one() {
    let _turn = connection_turn().await;
    let Some(view) = staged("governance") else {
        return;
    };
    let artifact = deployment(&std::fs::read(view).unwrap(), "a.svg");
    let node = FakeDeployment::serving("governance", &artifact);
    *node.status.lock().unwrap() = serde_json::json!({"module_status": {"modules": []}});
    let client = fake_node(node.clone()).await;
    let seat = fresh("governance");
    let settled = |loads: Loads| tokio::task::spawn_blocking(move || loads.joined());
    settled(connected(&client)).await.unwrap();
    assert!(matches!(
        seat.lock().unwrap().slot,
        Slot::Failed(Failure::NotListed(_))
    ));
    // deployed since: Retry asks again at once, and the tab says so at once
    node.deploy("governance", &artifact);
    let hold = hold_status(&node);
    let loads = retry("governance");
    node.held.notified().await;
    assert!(matches!(seat.lock().unwrap().slot, Slot::Loading));
    hold.notify_one();
    settled(loads).await.unwrap();
    // a view that stopped is named Trapped, and Retry seats a fresh instance
    let stopped = {
        let mut locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &mut locked.slot else {
            panic!("retry did not seat the view");
        };
        guest.fault = Some("all fuel consumed by WebAssembly".into());
        guest.alive.clone()
    };
    settled(retry("governance")).await.unwrap();
    {
        let locked = seat.lock().unwrap();
        let Slot::Ready(guest) = &locked.slot else {
            panic!("retry did not reseat the stopped view");
        };
        assert!(guest.fault.is_none());
        assert!(!Arc::ptr_eq(&stopped, &guest.alive));
    }
    for module in crate::backend::view_source::MODULE_OWNED {
        unseat(module);
    }
}

/// #110: a seat whose layer mounts only once it draws — the palette — never
/// ran, so never claimed its chord, so the chord found no holder. A seated
/// view no layer draws is turned all the same: it claims its chord, and the
/// chord opens it.
#[test]
fn an_unmounted_seated_view_claims_its_chord_and_the_chord_opens_it() {
    let Some(path) = staged("palette") else {
        return;
    };
    let _turn = super::tests::blocking_connection_turn();
    let seat = fresh("palette");
    let guest = Guest::load_from("palette", &path).expect("the palette loads");
    seat.lock().unwrap().slot = Slot::Ready(Box::new(guest));
    let mut cx = crate::frame_probe::headless_context();
    let view = cx.update(|cx| cx.new(|_| NativeModuleView::new("palette")));
    let spec = palette_view(false, true, "walk#00");
    // the shell sets the props on every render and mounts no layer for a
    // view that draws nothing
    cx.update(|cx| view.update(cx, |view, cx| view.set_props(spec.props, cx)));
    cx.run_until_parked();
    assert_eq!(chord_holder("cmd-k"), Some("palette"));
    assert!(
        !cx.update(|cx| view.read(cx).draws()),
        "a closed palette draws"
    );
    assert!(cx.update(|cx| view.update(cx, |view, cx| view.chord("cmd-k", cx))));
    assert!(
        cx.update(|cx| view.read(cx).draws()),
        "the chord did not open the palette"
    );
    unseat("palette");
}
