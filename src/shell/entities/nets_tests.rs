//! Two networks at once: a pop-out on A keeps its seat, its props and A's
//! blocks while the console switches to B, its panes are sized by B's
//! views, and a float left on A stays out rather than ending its pane.
use super::tests::{desk_of, entities, programs, source, status};
use super::{Entities, Net, Screen, Windows};
use crate::runtime::WindowKey;
use crate::shell::WindowKind;
use gpui_kit::{Bounds, TestAppContext, point, px, size};

const A: &str = "http://127.0.0.1:1";
const B: &str = "http://127.0.0.1:2";

struct HandedBack;

impl Drop for HandedBack {
    fn drop(&mut self) {
        crate::runtime::in_hand().hand_notifiers();
    }
}

struct Two {
    app: Entities,
    console: WindowKey,
    popped: WindowKey,
    instance: u64,
    a: Net,
    b: Net,
}

fn node(network: &str, height: u64) -> crate::backend::NodeStatus {
    crate::backend::NodeStatus {
        network: network.into(),
        ..status(height)
    }
}

fn net_of(app: &Entities, key: WindowKey, cx: &TestAppContext) -> Net {
    app.windows.read_with(cx, |windows, _| {
        windows.own(key).expect("its window").net.clone()
    })
}

fn chain_of(net: &Net, cx: &TestAppContext) -> String {
    net.session
        .read_with(cx, |session, _| session.get().chain.clone())
}

fn props_of(net: &Net, cx: &TestAppContext) -> Vec<u8> {
    net.seats.read_with(cx, |seats, _| seats.props().to_vec())
}

fn holder(app: &Entities, instance: u64, cx: &TestAppContext) -> WindowKey {
    app.windows.read_with(cx, |windows, cx| {
        let held = windows.by_window().iter().find(|(_, own)| {
            let layout = own.desk.read(cx).get();
            layout.panes.iter().any(|pane| pane.instance == instance)
        });
        *held.expect("the pane is on a desk").0
    })
}

fn two(program: &'static str, cx: &mut TestAppContext) -> Two {
    cx.executor().allow_parking();
    let app = cx.update(|cx| {
        gpui_kit::init(cx);
        let app = entities(cx);
        Windows::forget_closed(&app.windows, cx);
        app
    });
    let at = Bounds::new(point(px(0.), px(0.)), size(px(1280.), px(800.)));
    let console = app.windows.update(cx, |windows, cx| {
        windows.open(WindowKind::Console, Some(at), None, cx)
    });
    cx.run_until_parked();
    let on_a = source(|_| Ok(node("nets-a", 1))).0;
    app.session.update(cx, |session, cx| {
        session.connect_through(A.into(), on_a, cx)
    });
    cx.run_until_parked();
    let a = net_of(&app, console, cx);
    assert_eq!(
        chain_of(&a, cx),
        crate::runtime::chain_of(&node("nets-a", 1)),
        "the console did not reach A"
    );
    a.screen
        .update(cx, |screen, cx| _ = screen.set(Screen::Desk, cx));
    desk_of(&app, console, cx).update(cx, |desk, cx| {
        desk.resize((1280., 764.), cx);
        desk.seed(program, cx);
    });
    let popped = app
        .windows
        .update(cx, |windows, cx| {
            windows.pop_out(console, 0, None, None, cx)
        })
        .expect("popped out");
    cx.run_until_parked();
    let instance = desk_of(&app, popped, cx).read_with(cx, |desk, _| desk.get().panes[0].instance);
    let on_b = source(|_| Ok(node("nets-b", 1))).0;
    app.windows
        .update(cx, |windows, cx| windows.switch_through(B.into(), on_b, cx));
    cx.run_until_parked();
    let b = net_of(&app, console, cx);
    Two {
        app,
        console,
        popped,
        instance,
        a,
        b,
    }
}

#[gpui_kit::test]
fn a_popout_keeps_its_network_while_the_console_switches(cx: &mut TestAppContext) {
    const PROGRAM: &str = "nets-chat";
    let _seat = crate::backend::seat_serial();
    let _alone = crate::runtime::alone_for_test();
    let _handed = HandedBack;
    let Two {
        app,
        console,
        popped,
        instance,
        a,
        b,
    } = two(PROGRAM, cx);
    let chain_a = crate::runtime::chain_of(&node("nets-a", 1));
    let chain_b = crate::runtime::chain_of(&node("nets-b", 1));
    let seat = a.seats.read_with(cx, |seats, _| seats.seat(instance));
    assert!(seat.is_some(), "the pop-out has no seat on A");
    assert!(b != a, "the console stayed on A's network");
    assert_eq!(chain_of(&b, cx), chain_b, "the console did not reach B");
    assert_eq!(b.network.reached(), Some(("nets-b".to_owned(), chain_b)));
    assert!(
        b.network.notified(),
        "the notifiers did not follow the console"
    );
    assert!(!a.network.notified(), "the notifiers stayed on A");
    assert!(
        !programs(&app, console, cx).contains(&PROGRAM),
        "B's desk shows A's pane"
    );
    assert!(net_of(&app, popped, cx) == a, "the pop-out left A");
    assert_eq!(chain_of(&a, cx), chain_a, "A's session left its chain");
    assert_eq!(
        a.network.reached(),
        Some(("nets-a".to_owned(), chain_a)),
        "A's node was let go"
    );
    assert_eq!(
        programs(&app, popped, cx),
        [PROGRAM],
        "the pop-out was cleared"
    );
    assert!(a.network.seated(PROGRAM, instance));
    assert!(
        !b.network.seated(PROGRAM, instance),
        "the pop-out was seated on B"
    );
    let props = props_of(&a, cx);
    assert_ne!(props_of(&b, cx), props, "B's seats were handed A's props");

    let (a_checked, b_checked) = (a.network.checked(), b.network.checked());
    a.session.update(cx, |session, cx| {
        session.status_answered(Ok(node("nets-a", 2)), cx)
    });
    cx.run_until_parked();
    assert!(
        a.seats.read_with(cx, |seats, _| seats.seat(instance)) == seat,
        "the pop-out's seat was made again"
    );
    assert_eq!(props_of(&a, cx), props, "the pop-out's props moved");
    assert_eq!(
        a.chain.read_with(cx, |chain, _| chain.height),
        2,
        "A's block was not heard"
    );
    assert_eq!(
        a.network.checked(),
        a_checked + 1,
        "A's block checked none of A's deployments"
    );
    assert_eq!(
        b.network.checked(),
        b_checked,
        "A's block checked B's deployments"
    );
}

#[gpui_kit::test]
fn the_consoles_panes_are_sized_by_its_own_networks_views(cx: &mut TestAppContext) {
    const PROGRAM: &str = "nets-sized";
    const ON_B: &str = "nets-sized-b";
    let _seat = crate::backend::seat_serial();
    let _alone = crate::runtime::alone_for_test();
    let _handed = HandedBack;
    let Two {
        app, console, b, ..
    } = two(PROGRAM, cx);
    let desk = desk_of(&app, console, cx);
    desk.update(cx, |desk, cx| {
        desk.resize((1280., 764.), cx);
        desk.seed(ON_B, cx);
    });
    cx.run_until_parked();
    let pane = desk.read_with(cx, |desk, _| desk.get().panes[0].clone());
    assert_eq!(pane.program, ON_B);
    assert!(
        b.network.seated(ON_B, pane.instance),
        "B's pane was not seated on B"
    );
    b.network.compiled_for_test(ON_B, 900);
    assert_eq!(pane.min_width(), 902., "B's pane was not sized by B's view");
    assert_eq!(
        crate::shell::windows::popout_size(ON_B, pane.instance).width,
        px(900.),
        "B's pop-out minimum was not B's view's"
    );
}

#[gpui_kit::test]
fn a_float_left_on_another_network_stays_out_as_a_popout(cx: &mut TestAppContext) {
    const PROGRAM: &str = "nets-call";
    let _seat = crate::backend::seat_serial();
    let _alone = crate::runtime::alone_for_test();
    let _handed = HandedBack;
    let Two {
        app, instance, a, ..
    } = two(PROGRAM, cx);
    let seat = a.seats.read_with(cx, |seats, _| seats.seat(instance));
    assert!(seat.is_some(), "the pop-out has no seat on A");
    let float = |cx: &mut TestAppContext| {
        let (body, min) = (size(px(320.), px(200.)), size(px(320.), px(200.)));
        app.windows.update(cx, |windows, cx| {
            windows.float(instance, body, min, false, cx)
        });
        cx.run_until_parked();
        let key = holder(&app, instance, cx);
        let floated = app
            .windows
            .read_with(cx, |windows, _| windows.float_min(key));
        assert!(floated.is_some(), "the pane did not float");
        key
    };
    let floats = |key, cx: &TestAppContext| {
        app.windows
            .read_with(cx, |windows, _| windows.float_min(key).is_some())
    };

    let key = float(cx);
    assert!(net_of(&app, key, cx) == a, "the float left A");
    app.windows
        .update(cx, |windows, cx| windows.unfloat(instance, cx));
    cx.run_until_parked();
    assert_eq!(
        holder(&app, instance, cx),
        key,
        "unfloat took the pane off A"
    );
    assert!(!floats(key, cx), "unfloat left it a float");

    app.windows
        .update(cx, |windows, cx| windows.pop_in(key, cx));
    cx.run_until_parked();
    assert_eq!(
        holder(&app, instance, cx),
        key,
        "Pop In took the pane off A"
    );
    let said = app.toast.read_with(cx, |toast, _| toast.get().clone());
    assert!(
        said.contains("nets-a"),
        "Pop In said nothing of why: {said:?}"
    );

    let key = float(cx);
    let handle = app
        .windows
        .read_with(cx, |windows, _| windows.handles()[&key]);
    let windows = app.windows.clone();
    let closes = cx
        .update(|cx| {
            handle.update(cx, |_, window, cx| {
                windows.update(cx, |windows, cx| windows.floated_closes(key, window, cx))
            })
        })
        .expect("its window");
    cx.run_until_parked();
    assert!(!closes, "its window closed with the pane");
    assert_eq!(holder(&app, instance, cx), key);
    assert!(!floats(key, cx), "closing left it a float");
    assert!(
        a.seats.read_with(cx, |seats, _| seats.seat(instance)) == seat,
        "the pane's seat went"
    );
}
