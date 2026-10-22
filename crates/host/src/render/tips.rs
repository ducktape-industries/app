//! A view's tooltip, drawn in its pane's portal while the window's pointer
//! router holds it open (`pointer.rs`): at the pointer, fitted to the pane.
use super::*;
use gpui_kit::{Anchor, Edges};

pub(super) struct Tip {
    request: u32,
    character_index: Option<u32>,
    at: Point<Pixels>,
    drawn: Option<(Arc<wire::Node>, gpui_kit::AnyView)>,
}

impl ViewTree {
    pub(super) fn tip_show(
        &mut self,
        request: u32,
        character_index: Option<u32>,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.tip = Some(Tip {
            request,
            character_index,
            at,
            drawn: None,
        });
        cx.emit(wire::Event::TooltipRequest {
            request,
            character_index,
        });
        cx.notify();
    }

    pub(super) fn tip_hide(&mut self, cx: &mut Context<Self>) {
        if self.tip.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn tip_element(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tip = self.tip.as_mut()?;
        let held = self.tooltips.get(&tip.request)?;
        let another_character =
            tip.character_index.is_some() && held.character_index != tip.character_index;
        if another_character {
            return None;
        }
        let view = match &tip.drawn {
            Some((content, view)) if Arc::ptr_eq(content, &held.content) => view.clone(),
            _ => {
                let tree = Tree {
                    root: (*held.content).clone(),
                    styles: held.styles.clone(),
                };
                let shared = (self.pictures.clone(), self.images.clone());
                let view =
                    super::tooltip_containment::build(cx.entity().downgrade(), tree, shared, cx);
                tip.drawn = Some((held.content.clone(), view.clone()));
                view
            }
        };
        let surface = super::pointer::surface(cx.entity_id(), view.into_any_element());
        Some(
            super::anchored::Fitted {
                children: vec![surface],
                anchor: Anchor::TopLeft,
                position: Some(tip.at + point(px(1.), px(1.))),
                local: false,
                offset: Point::default(),
                margin: Edges::default(),
            }
            .into_any_element(),
        )
    }
}
