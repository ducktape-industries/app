use super::*;
use crate::view_tree::native_id;

#[derive(Clone)]
pub(super) struct Choice {
    pub(super) index: u32,
    pub(super) label: String,
}

impl SearchableListItem for Choice {
    type Value = u32;
    fn title(&self) -> SharedString {
        self.label.clone().into()
    }
    fn value(&self) -> &u32 {
        &self.index
    }
}

pub(super) type SearchQueryFn = Box<dyn Fn(&str, &mut App)>;

pub(super) struct PickerChoices {
    pub(super) items: SearchableVec<Choice>,
    pub(super) query: SearchQueryFn,
}

impl SearchableListDelegate for PickerChoices {
    type Item = Choice;
    fn items_count(&self, section: usize) -> usize {
        self.items.items_count(section)
    }
    fn item(&self, index: IndexPath) -> Option<&Choice> {
        self.items.item(index)
    }
    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.items.position(value)
    }
    fn perform_search(&mut self, query: &str, window: &mut Window, cx: &mut App) -> Task<()> {
        (self.query)(query, cx);
        self.items.perform_search(query, window, cx)
    }
}

pub(super) struct Picker {
    pub(super) state: Entity<SelectState<PickerChoices>>,
    pub(super) options: Vec<String>,
    pub(super) selected: Option<u32>,
    pub(super) handler: u32,
    pub(super) input: Option<u32>,
    pub(super) reset: Option<(String, u64)>,
    pub(super) _subscription: Subscription,
}

impl ViewTree {
    pub(super) fn picker(
        &mut self,
        node: &wire::Node,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (id, options, selected, handler, placeholder, style, reset, input) = match node {
            wire::Node::PickList {
                id,
                options,
                selected,
                on_select,
                placeholder,
                style,
                ..
            } => (
                id,
                options,
                *selected,
                *on_select,
                placeholder.as_deref().unwrap_or_default(),
                style,
                None,
                None,
            ),
            wire::Node::ComboBox {
                id,
                state_key,
                options,
                selected,
                on_select,
                placeholder,
                style,
                reset,
                settings,
                ..
            } => (
                id,
                options,
                *selected,
                *on_select,
                placeholder.as_str(),
                style,
                Some((state_key.clone(), *reset)),
                settings.input,
            ),
            _ => unreachable!(),
        };
        let path = self.authored_path.clone();
        if self
            .pickers
            .get(&path)
            .is_some_and(|picker| picker.reset != reset)
        {
            self.pickers.remove(&path);
        }
        let weak = cx.entity().downgrade();
        let choices = || {
            let items = options
                .iter()
                .enumerate()
                .map(|(index, label)| Choice {
                    index: index as u32,
                    label: label.clone(),
                })
                .collect::<Vec<_>>();
            let weak = weak.clone();
            let key = path.clone();
            PickerChoices {
                items: SearchableVec::new(items),
                query: Box::new(move |query, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if let Some(handler) =
                            this.pickers.get(&key).and_then(|picker| picker.input)
                        {
                            cx.emit(wire::Event::Input {
                                handler,
                                text: query.to_owned(),
                            });
                        }
                    });
                }),
            }
        };
        let index = selected.map(|index| IndexPath::new(index as usize));
        if !self.pickers.contains_key(&path) {
            let state = cx.new(|cx| {
                SelectState::new(choices(), index, window, cx).searchable(reset.is_some())
            });
            let route = path.clone();
            let subscription = cx.subscribe_in(&state, window, move |this, _, event, _, cx| {
                let SelectEvent::Confirm(Some(index)) = event else {
                    return;
                };
                let Some(picker) = this.pickers.get_mut(&route) else {
                    return;
                };
                picker.selected = Some(*index);
                cx.emit(wire::Event::Select {
                    handler: picker.handler,
                    index: *index,
                });
            });
            self.pickers.insert(
                path.clone(),
                Picker {
                    state,
                    options: options.to_vec(),
                    selected,
                    handler,
                    input,
                    reset,
                    _subscription: subscription,
                },
            );
        }
        let picker = self.pickers.get_mut(&path).expect("picker inserted");
        picker.handler = handler;
        picker.input = input;
        if picker.options != *options {
            picker.options = options.to_vec();
            picker
                .state
                .update(cx, |state, cx| state.set_items(choices(), window, cx));
        }
        if picker.selected != selected {
            picker.selected = selected;
            let state = picker.state.downgrade();
            let route = path.clone();
            // A wire index identifies a value in the full option set, not a
            // row in the filtered menu. Native value projection clears search
            // synchronously, so run it after releasing this render borrow.
            window.defer(cx, move |window, cx| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                let current = weak
                    .read_with(cx, |tree, _| {
                        tree.pickers.get(&route).is_some_and(|picker| {
                            picker.state.entity_id() == state.entity_id()
                                && picker.selected == selected
                        })
                    })
                    .unwrap_or(false);
                if !current {
                    return;
                }
                state.update(cx, |state, cx| match selected {
                    Some(value) => state.set_selected_value(&value, window, cx),
                    None => state.set_selected_index(None, window, cx),
                });
            });
        }
        let mut select = Select::new(&picker.state)
            .id(native_id(id))
            .placeholder(placeholder.to_owned());
        // the kit draws the picker's node itself; the wire `label` names it
        if let Some(name) = accessible(node).name {
            select = select.accessibility_label(name);
        }
        div()
            .relative()
            .refine_style(style)
            .child(select)
            .child(self.measure(&path, cx))
            .into_any_element()
    }
}
