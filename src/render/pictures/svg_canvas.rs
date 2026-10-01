use super::*;
use gpui_kit::relative;

/// The SVG road for commands gpui cannot paint itself. A canvas has no wire
/// identity, so nothing measures it across frames: the picture is drawn at
/// prepaint, when its own bounds are known, and laid out as a root there.
pub(super) struct SvgCanvas {
    pub(super) commands: Vec<wire::CanvasCommand>,
}

impl IntoElement for SvgCanvas {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SvgCanvas {
    type RequestLayoutState = ();
    type PrepaintState = AnyElement;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = gpui_kit::Style {
            size: size(relative(1.).into(), relative(1.).into()),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let width = f32::from(bounds.size.width).max(1.);
        let height = f32::from(bounds.size.height).max(1.);
        let svg_bytes = canvas_svg(&self.commands, width, height);
        let mut picture = guarded_svg_paint(
            SvgPaintSource::data(&svg_bytes),
            img(Arc::new(Image::from_bytes(ImageFormat::Svg, svg_bytes)))
                .size_full()
                .object_fit(ObjectFit::Fill),
        )
        .into_any_element();
        picture.layout_as_root(bounds.size.map(Into::into), window, cx);
        picture.prepaint_at(bounds.origin, window, cx);
        picture
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        picture: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        picture.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::tests::{Seat, named_id};

    fn boxed(id: Option<&str>, width: f32, height: f32, children: Vec<wire::Node>) -> wire::Node {
        wire::Node::Container(view_wire::ContainerNode {
            id: id.map(named_id),
            style: div().w(px(width)).h(px(height)).style().clone(),
            interactivity: Default::default(),
            children,
        })
    }

    /// The tree in a cached seat, as on the desk: it renders again only when
    /// it asks to. Three frames to settle, five more, and the renders those
    /// five cost — a tree at rest costs none.
    fn redraws_at_rest(
        root: wire::Node,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (u64, Entity<ViewTree>, gpui_kit::VisualTestContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
            Seat(cx.new(|_| ViewTree::new(root)))
        });
        let seat = window.root(cx).unwrap();
        let tree = seat.read_with(cx, |seat, _| seat.0.clone());
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let frame = |native: &mut gpui_kit::VisualTestContext| {
            native.update(|window, cx| {
                seat.update(cx, |_, cx| cx.notify());
                window.draw(cx).clear(cx);
            });
            native.run_until_parked();
        };
        for _ in 0..3 {
            frame(&mut native);
        }
        let settled = tree.read_with(&native, |tree, _| tree.renders);
        for _ in 0..5 {
            frame(&mut native);
        }
        let after = tree.read_with(&native, |tree, _| tree.renders);
        (after - settled, tree, native)
    }

    #[gpui_kit::test]
    fn a_canvas_leaves_its_parents_measure_alone(cx: &mut gpui_kit::TestAppContext) {
        // a Path shape takes the SVG road, the one that needs the canvas's size
        let drawing = wire::Node::Canvas {
            commands: vec![wire::CanvasCommand::Draw {
                shape: wire::CanvasShape::Path(vec![]),
                fill: Some(gpui_kit::rgb(0xff0000).into()),
                stroke: None,
                even_odd: false,
            }],
            style: div().w(px(50.)).h(px(50.)).style().clone(),
        };
        let root = boxed(Some("card"), 200., 100., vec![drawing]);
        let (redraws, tree, native) = redraws_at_rest(root, cx);
        assert_eq!(redraws, 0, "the two measures fight over one entry");
        let card = tree.read_with(&native, |tree, _| tree.measured_bounds(&[named_id("card")]));
        assert_eq!(card.map(|card| card.size), Some(size(px(200.), px(100.))));
    }
}
