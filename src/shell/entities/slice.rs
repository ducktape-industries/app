//! What every shell entity is built from: a value whose one write compares
//! first, the handle a view reads an entity through, and how an entity runs
//! work that needs the kernel's tokio runtime.
use gpui_kit::{App, Context, Entity, Subscription};
use std::future::Future;

/// A shared value. Its one write compares first: setting an equal value
/// notifies no observer, so a beat that moves nothing draws nothing.
pub(crate) struct Slice<T: PartialEq>(T);

impl<T: PartialEq + 'static> Slice<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(value)
    }

    pub(crate) fn get(&self) -> &T {
        &self.0
    }

    /// Takes `value` and notifies when it differs from the one held;
    /// `true` when it did.
    pub(crate) fn set(&mut self, value: T, cx: &mut Context<Self>) -> bool {
        if self.0 == value {
            return false;
        }
        self.0 = value;
        cx.notify();
        true
    }

    /// A field-wise edit, compared as a whole once `edit` has run.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the desk's resize and seed edit the layout in place from s11"
        )
    )]
    pub(crate) fn edit(&mut self, edit: impl FnOnce(&mut T), cx: &mut Context<Self>) -> bool
    where
        T: Clone,
    {
        let mut next = self.0.clone();
        edit(&mut next);
        self.set(next, cx)
    }
}

/// A view's handle on an entity it reads. Making one subscribes the view,
/// so a notify of the entity re-renders it: a cached view that reads an
/// entity without observing it is drawn stale. A view holds these, never a
/// bare `Entity`.
pub(crate) struct Observed<T: 'static> {
    entity: Entity<T>,
    _observing: Subscription,
}

impl<T: 'static> Observed<T> {
    pub(crate) fn new<V: 'static>(entity: &Entity<T>, cx: &mut Context<V>) -> Self {
        Self {
            entity: entity.clone(),
            _observing: cx.observe(entity, |_, _, cx| cx.notify()),
        }
    }

    pub(crate) fn read<'a>(&self, cx: &'a App) -> &'a T {
        self.entity.read(cx)
    }
}

/// Runs `work` on the window thread with the views kernel's tokio runtime
/// entered for every poll (as `Desktop::start` runs the reducer's tasks),
/// then hands its output to `land` on the entity. Dropping the task drops
/// the work; an entity already gone takes nothing.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the shell's entities run their tasks through this from s10 on"
    )
)]
pub(crate) fn spawn_on_runtime<E: 'static, R: 'static>(
    cx: &mut Context<E>,
    work: impl Future<Output = R> + 'static,
    land: impl FnOnce(&mut E, R, &mut Context<E>) + 'static,
) -> gpui_kit::Task<()> {
    let runtime = crate::runtime::handle();
    cx.spawn(async move |this, cx| {
        let mut work = std::pin::pin!(work);
        let output = std::future::poll_fn(|context| {
            let _runtime = runtime.enter();
            work.as_mut().poll(context)
        })
        .await;
        let _ = this.update(cx, |this, cx| land(this, output, cx));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext as _, TestAppContext};
    use std::cell::Cell;
    use std::rc::Rc;

    /// Counts `entity`'s notifies while the subscription lives.
    fn notifies<T: 'static>(
        entity: &Entity<T>,
        cx: &mut TestAppContext,
    ) -> (Rc<Cell<usize>>, Subscription) {
        let seen = Rc::new(Cell::new(0));
        let count = seen.clone();
        let observing = cx.update(|cx| cx.observe(entity, move |_, _| count.set(count.get() + 1)));
        (seen, observing)
    }

    #[gpui_kit::test]
    fn an_equal_value_notifies_nothing_and_a_changed_one_once(cx: &mut TestAppContext) {
        let slice = cx.new(|_| Slice::new(1));
        let (seen, _observing) = notifies(&slice, cx);
        assert!(!slice.update(cx, |slice, cx| slice.set(1, cx)));
        assert_eq!(seen.get(), 0, "an equal value notified");
        assert!(slice.update(cx, |slice, cx| slice.set(2, cx)));
        assert_eq!(seen.get(), 1, "a changed value did not notify once");
        assert!(!slice.update(cx, |slice, cx| slice.edit(|value| *value *= 1, cx)));
        assert_eq!(seen.get(), 1, "an edit that changed nothing notified");
        assert!(slice.update(cx, |slice, cx| slice.edit(|value| *value += 1, cx)));
        assert_eq!(
            seen.get(),
            2,
            "an edit that changed the value did not notify once"
        );
        assert_eq!(slice.read_with(cx, |slice, _| *slice.get()), 3);
    }

    struct Reader(Observed<Slice<u8>>);

    #[gpui_kit::test]
    fn making_an_observed_subscribes_its_view(cx: &mut TestAppContext) {
        let slice = cx.new(|_| Slice::new(0));
        let reader = cx.new(|cx| Reader(Observed::new(&slice, cx)));
        let (seen, _observing) = notifies(&reader, cx);
        slice.update(cx, |slice, cx| slice.set(0, cx));
        assert_eq!(
            seen.get(),
            0,
            "the reader was told of a value that did not move"
        );
        slice.update(cx, |slice, cx| slice.set(7, cx));
        assert_eq!(seen.get(), 1, "the slice moved and its reader was not told");
        assert_eq!(
            reader.read_with(cx, |reader, cx| *reader.0.read(cx).get()),
            7
        );
    }

    #[gpui_kit::test]
    fn work_spawned_on_the_runtime_polls_inside_it(cx: &mut TestAppContext) {
        let slice = cx.new(|_| Slice::new(false));
        let _work = slice.update(cx, |_, cx| {
            spawn_on_runtime(
                cx,
                async { tokio::runtime::Handle::try_current().is_ok() },
                |slice, inside, cx| {
                    slice.set(inside, cx);
                },
            )
        });
        cx.run_until_parked();
        assert!(
            slice.read_with(cx, |slice, _| *slice.get()),
            "the work ran outside the kernel's runtime"
        );
    }
}
