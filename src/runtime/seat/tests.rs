use super::*;

/// The table on [`Mounted::reload_due`], row by row.
#[test]
fn a_roster_read_reloads_by_code_node_and_hold_off() {
    let now = Instant::now();
    let code = [1; 32];
    let held = |code, next| Retry {
        code,
        next,
        gap: RETRY_FIRST,
    };
    let running = now + RETRY_FIRST;
    let up = now - RETRY_FIRST;
    // (retry, same_code, asked_of_this_node) -> load
    let table = [
        (None, false, false, true),
        (None, true, false, true),
        (None, false, true, true),
        (None, true, true, false),
        // the gap is up: the re-attempt at the same code it promised
        (Some(held(Some(code), up)), true, true, true),
        (Some(held(Some(code), up)), false, true, true),
        // running: this very code waits, whatever else moved
        (Some(held(Some(code), running)), false, false, false),
        (Some(held(Some(code), running)), true, true, false),
        // running for another code, or for none: nothing held off here
        (Some(held(Some([2; 32]), running)), true, true, true),
        (Some(held(None, running)), true, true, true),
    ];
    for (nth, (retry, same_code, asked_of_this_node, load)) in table.into_iter().enumerate() {
        let seat = Mounted::seat();
        seat.lock().unwrap().retry = retry;
        assert_eq!(
            seat.lock()
                .unwrap()
                .reload_due(same_code, asked_of_this_node, code, now),
            load,
            "row {nth}"
        );
    }
}

/// A view that fails to load is held off under the blob the roster names,
/// not under its own section's hash: the two never match, and keying on
/// the section had every block reload the failed view while the hold-off
/// ran and skip it once the gap was up.
#[test]
fn a_failed_view_is_held_off_under_the_code_the_roster_names() {
    let code = [1; 32];
    let section = [9; 32];
    let seat = Mounted::seat();
    let mut locked = seat.lock().unwrap();
    locked.load_failed(
        "seat-tests-failed",
        None,
        Some(code),
        Unloaded {
            hash: Some(section),
            failure: Failure::Refused("not a view".into()),
        },
    );
    assert!(matches!(locked.slot, Slot::Failed(_)));
    let now = Instant::now();
    assert!(
        !locked.reload_due(true, true, code, now),
        "held off: the same code is not asked again this block"
    );
    assert!(
        locked.reload_due(true, true, code, now + RETRY_FIRST * 2),
        "the gap is up: asked again"
    );
    assert!(
        locked.reload_due(true, true, [2; 32], now),
        "another code is not held off"
    );

    // failing again on the same code widens the gap; a failure before any
    // candidate holds nothing off
    let held_off = locked.retry.take();
    locked.load_failed(
        "seat-tests-failed",
        held_off,
        Some(code),
        Unloaded {
            hash: Some(section),
            failure: Failure::Refused("not a view".into()),
        },
    );
    assert_eq!(locked.retry.as_ref().unwrap().gap, RETRY_FIRST * 2);
    let held_off = locked.retry.take();
    locked.load_failed(
        "seat-tests-failed",
        held_off,
        Some(code),
        Unloaded {
            hash: None,
            failure: Failure::Unreachable("no node".into()),
        },
    );
    assert_eq!(locked.retry.as_ref().unwrap().code, None);
    assert!(locked.reload_due(true, true, code, now));
}
