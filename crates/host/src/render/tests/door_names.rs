//! Each node a seated view draws carries its own element id, under its
//! program, as its accessibility id.
use super::*;

struct Seated(Entity<ViewTree>);

impl Render for Seated {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(crate::render::host_id(format!(
                "{}chat",
                crate::render::VIEW_MARK
            )))
            .size_full()
            .child(self.0.clone())
    }
}

#[gpui_kit::test]
fn every_node_of_a_seated_view_is_named_by_its_own_id_under_its_program(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::init);
    let [image, svg] = picture(Some("a duck"));
    let root = axis_container(
        "root",
        Axis::Column,
        [
            text("greeting", "hello"),
            input("Room name", false, false),
            area(
                "composer",
                Some("Message"),
                "hi",
                gpui_kit::StyleRefinement::default(),
            ),
            button("send", "Send"),
            rich(),
            image,
            svg,
        ],
    );
    let window = cx.open_window(size(px(600.), px(500.)), |_, cx| {
        let tree = cx.new(|_| {
            ViewTree::new(root).with_perf_key(crate::perf::Key::View {
                program: "chat",
                instance: 0,
            })
        });
        Seated(tree)
    });
    let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let (named, misnamed) = native.update(|window, cx| {
        window.activate_a11y();
        crate::render::draw_frame(window, cx);
        crate::render::draw_frame(window, cx);
        let named: Vec<String> = window
            .a11y_tree()
            .unwrap()
            .nodes
            .iter()
            .filter_map(|(_, node)| node.author_id().map(str::to_owned))
            .collect();
        (named, crate::a11y::misnamed(window))
    });
    assert!(misnamed.is_empty(), "\n{}\n", misnamed.join("\n"));
    assert!(named.iter().any(|id| id == "view/chat/send"), "{named:?}");
}
