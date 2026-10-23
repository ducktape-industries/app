//! `host.showing` and `host.screen`: the route each view names as the one
//! it shows, and, for each program on each chain, the routes its views show
//! on screen (`host.visible`), which its notifier reads to leave alone what
//! the person is looking at. The app knows nothing of any route: it keeps
//! what each view names, while that view is on screen. Two networks run
//! one program apart (another listed network's notifier, `notifier::follow`):
//! a route on screen is its own chain's alone.
use super::*;
use std::collections::BTreeMap;

type Routes = tokio::sync::watch::Sender<Vec<String>>;

#[derive(Default)]
struct Screen {
    shown: BTreeMap<(&'static str, u64), (String, String)>,
    programs: HashMap<(String, &'static str), Routes>,
}

impl Screen {
    fn routes(&mut self, chain: &str, program: &'static str) -> &Routes {
        self.programs
            .entry((chain.to_owned(), program))
            .or_insert_with(|| tokio::sync::watch::channel(Vec::new()).0)
    }
}

fn screen() -> std::sync::MutexGuard<'static, Screen> {
    static SCREEN: OnceLock<Mutex<Screen>> = OnceLock::new();
    SCREEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(in crate::runtime) fn shown(
    chain: &str,
    program: &'static str,
    instance: u64,
    route: Option<String>,
) {
    let mut screen = screen();
    let before = match route {
        Some(route) => screen
            .shown
            .insert((program, instance), (chain.to_owned(), route)),
        None => screen.shown.remove(&(program, instance)),
    };
    let now = screen.shown.get(&(program, instance)).cloned();
    if before == now {
        return;
    }
    let mut chains: Vec<String> = [before, now]
        .into_iter()
        .flatten()
        .map(|(on, _)| on)
        .collect();
    chains.dedup();
    for chain in chains {
        let mut routes: Vec<String> = screen
            .shown
            .iter()
            .filter(|((shown, _), (on, _))| *shown == program && *on == chain)
            .map(|(_, (_, route))| route.clone())
            .collect();
        routes.sort();
        routes.dedup();
        screen.routes(&chain, program).send_replace(routes);
    }
}

pub(super) fn follow(view: &mut View, id: u64) {
    let mut routes = screen()
        .routes(&view.connection.chain, view.program)
        .subscribe();
    spawn_host_subscription(view, id, move |mut items| async move {
        loop {
            let now = routes.borrow_and_update().clone();
            if !items.send(Ok(methods::encode(&now))).await || routes.changed().await.is_err() {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(view: &View, id: u64) -> Vec<Vec<String>> {
        let deadline = rt::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut items = Vec::new();
        while items.is_empty() && rt::time::Instant::now() < deadline {
            let mut events = Vec::new();
            view.replies.drain_into(&mut events);
            items.extend(events.into_iter().filter_map(|event| match event {
                wire::Event::Response {
                    id: answered,
                    result: Ok(item),
                    ..
                } if answered == id => methods::decode::<Vec<String>>(&item).ok(),
                _ => None,
            }));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        items
    }

    #[test]
    fn the_screen_lists_what_the_programs_views_show_on_screen() {
        let program = "screen-tests";
        let mut notifier = crate::runtime::kernel::tests::view();
        notifier.program = program;
        assert!(answer(
            &mut notifier,
            Capability::Host,
            "screen",
            1,
            &[],
            &None
        ));
        assert_eq!(heard(&notifier, 1), [Vec::<String>::new()]);

        let mut pane = crate::runtime::kernel::tests::view();
        pane.program = program;
        pane.instance = 7;
        let route = methods::encode(&"general".to_owned());
        assert!(answer(
            &mut pane,
            Capability::Host,
            "showing",
            2,
            &route,
            &None
        ));
        pane.set_visible(true);
        assert_eq!(heard(&notifier, 1), [vec!["general".to_owned()]]);

        pane.set_visible(false);
        assert_eq!(heard(&notifier, 1), [Vec::<String>::new()]);
        pane.set_visible(true);
        assert_eq!(heard(&notifier, 1), [vec!["general".to_owned()]]);
        drop(pane);
        assert_eq!(heard(&notifier, 1), [Vec::<String>::new()]);
    }

    #[test]
    fn one_route_on_two_chains_stands_apart() {
        let program = "screen-tests-chains";
        let on = |chain: &str, instance| {
            let mut view = crate::runtime::kernel::tests::view();
            view.program = program;
            view.instance = instance;
            view.connection.chain = chain.into();
            view
        };
        let mut elsewhere = on("duckpond#02", crate::runtime::notifier::INSTANCE);
        assert!(answer(
            &mut elsewhere,
            Capability::Host,
            "screen",
            1,
            &[],
            &None
        ));
        assert_eq!(heard(&elsewhere, 1), [Vec::<String>::new()]);
        let mut here = on("garage#01", crate::runtime::notifier::INSTANCE);
        assert!(answer(&mut here, Capability::Host, "screen", 1, &[], &None));
        assert_eq!(heard(&here, 1), [Vec::<String>::new()]);

        let mut pane = on("garage#01", 7);
        let route = methods::encode(&"general".to_owned());
        assert!(answer(
            &mut pane,
            Capability::Host,
            "showing",
            2,
            &route,
            &None
        ));
        pane.set_visible(true);
        assert_eq!(heard(&here, 1), [vec!["general".to_owned()]]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut events = Vec::new();
        elsewhere.replies.drain_into(&mut events);
        assert!(events.is_empty(), "another chain's pane: {events:?}");
    }
}
