//! View entities belong to panes, including when a pane changes OS windows.
use super::*;

/// The program view shown in the pane with this layout `instance`
/// (`layout::Pane.instance`; the view counts its own, unrelated
/// `NativeModuleView.instance`).
pub(super) struct MountedPane {
    pub(super) module: &'static str,
    pub(super) view: Entity<crate::runtime::NativeModuleView>,
    /// The view's intents, to the model as `Message::ViewEvent`; dropping
    /// it unsubscribes.
    _route: gpui_kit::Subscription,
}

impl Desktop {
    /// A view for every pane the model has, and none for a pane it no
    /// longer has (told it is hidden first).
    pub(super) fn mount(&mut self, cx: &mut Context<Self>) {
        let wanted: BTreeMap<u64, &'static str> = self
            .state
            .layouts
            .values()
            .flat_map(|layout| &layout.panes)
            .filter(|pane| pane.is_view())
            .map(|pane| (pane.instance, pane.module))
            .collect();
        let gone: Vec<u64> = self
            .mounted
            .keys()
            .filter(|instance| !wanted.contains_key(instance))
            .copied()
            .collect();
        for (instance, module) in wanted {
            if self.mounted.contains_key(&instance) {
                continue;
            }
            let view = cx.new(|_| crate::runtime::NativeModuleView::new(module));
            let route = cx.subscribe(&view, move |model, _, event, cx| {
                model.dispatch(Message::ViewEvent(module, event.clone()), cx)
            });
            self.mounted.insert(
                instance,
                MountedPane {
                    module,
                    view,
                    _route: route,
                },
            );
        }
        for instance in gone {
            let Some(pane) = self.mounted.remove(&instance) else {
                continue;
            };
            let intents = pane.view.update(cx, |view, _| view.hide());
            for intent in intents {
                self.dispatch(Message::ViewEvent(pane.module, intent), cx);
            }
        }
    }
}
