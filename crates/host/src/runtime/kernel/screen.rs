//! `host.showing` and `host.screen`: the route each view names as the one
//! it shows, and, for each program, the routes its views show on screen
//! (`host.visible`), which its notifier reads to leave alone what the
//! person is looking at. The app knows nothing of any route: it keeps
//! what each view names, while that view is on screen.
use super::*;
use std::collections::BTreeMap;

type Routes = tokio::sync::watch::Sender<Vec<String>>;

/// What each view on screen shows, by its program and pane, and each
/// program's routes as `host.screen` hands them out. A leaf lock: nothing
/// else is taken while it is held.
#[derive(Default)]
struct Screen {
    shown: BTreeMap<(&'static str, u64), String>,
    programs: HashMap<&'static str, Routes>,
}

impl Screen {
    fn routes(&mut self, program: &'static str) -> &Routes {
        self.programs
            .entry(program)
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

/// The view of `program` in pane `instance` shows `route` on screen now, or
/// nothing (off screen, gone, or naming none): its program's routes move.
pub(in crate::runtime) fn shown(program: &'static str, instance: u64, route: Option<String>) {
    let mut screen = screen();
    let moved = match route {
        Some(route) => screen.shown.insert((program, instance), route.clone()) != Some(route),
        None => screen.shown.remove(&(program, instance)).is_some(),
    };
    if !moved {
        return;
    }
    let mut routes: Vec<String> = screen
        .shown
        .iter()
        .filter(|((shown, _), _)| *shown == program)
        .map(|(_, route)| route.clone())
        .collect();
    routes.sort();
    routes.dedup();
    // sent under the lock: two moves never land out of order
    screen.routes(program).send_replace(routes);
}

/// `host.screen`: the program's routes on screen now, then each time they
/// move.
pub(super) fn follow(view: &mut View, id: u64) {
    let mut routes = screen().routes(view.program).subscribe();
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

    /// The items `view` was handed on subscription `id` since the last
    /// look, decoded; taking them lets the next come.
    fn heard(view: &View, id: u64) -> Vec<Vec<String>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut items = Vec::new();
        while items.is_empty() && std::time::Instant::now() < deadline {
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

    /// A program's `host.screen` lists the routes its views name
    /// (`host.showing`) while they are on screen: nothing for a pane not in
    /// front, its route once it is, and nothing again once it goes.
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
}
