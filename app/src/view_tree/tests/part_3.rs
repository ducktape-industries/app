#[test]
fn a_button_reports_checked_expanded_and_its_description() {
    let mut node = button(wire::ButtonContent::Label("Bold".into()), None, Some(1));
    let wire::Node::Button {
        checked,
        expanded,
        description,
        ..
    } = &mut node
    else {
        unreachable!()
    };
    *checked = Some(true);
    *expanded = Some(false);
    *description = Some("Ctrl B".into());
    let heard = accessible(&node);
    assert_eq!(heard.toggled, Some(true));
    assert_eq!(heard.expanded, Some(false));
    assert_eq!(heard.description.as_deref(), Some("Ctrl B"));
    let wire::Node::Button { checked, .. } = &mut node else {
        unreachable!()
    };
    *checked = Some(false);
    assert_eq!(accessible(&node).toggled, Some(false));
}

#[test]
fn a_toggle_is_a_checkbox_or_a_switch_reporting_checked() {
    let toggle = |kind, label: &str, checked, on_toggle| wire::Node::Toggle {
        key: "t".into(),
        kind,
        label: label.into(),
        checked,
        on_toggle,
        width: None,
        style: Default::default(),
    };
    let checkbox = accessible(&toggle(wire::ToggleKind::Checkbox, "Notify", true, Some(1)));
    assert_eq!(
        checkbox,
        Accessible {
            role: Some(gpui_kit::Role::CheckBox),
            name: Some("Notify".into()),
            toggled: Some(true),
            ..Default::default()
        }
    );
    let switch = accessible(&toggle(wire::ToggleKind::Switch, "Mute", false, None));
    assert_eq!(switch.role, Some(gpui_kit::Role::Switch));
    assert_eq!(switch.toggled, Some(false));
    assert!(switch.disabled);
    assert_eq!(
        accessible(&toggle(wire::ToggleKind::Switch, "", false, None)).name,
        None
    );
}

#[test]
fn a_radio_reports_whether_it_is_the_selected_one() {
    let radio = |selected| wire::Node::Radio {
        key: "r".into(),
        label: "Weekly".into(),
        selected,
        on_select: 1,
        width: None,
        style: Default::default(),
    };
    assert_eq!(
        accessible(&radio(true)),
        Accessible {
            role: Some(gpui_kit::Role::RadioButton),
            name: Some("Weekly".into()),
            toggled: Some(true),
            ..Default::default()
        }
    );
    assert_eq!(accessible(&radio(false)).toggled, Some(false));
}

#[test]
fn a_slider_and_a_progress_report_their_value_in_range() {
    let slider = wire::Node::Slider {
        key: "s".into(),
        label: None,
        value: 30.,
        min: 0.,
        max: 100.,
        step: 5.,
        on_change: 1,
        on_release: None,
        axis: wire::Axis::Row,
        width: None,
        height: None,
        style: Default::default(),
    };
    assert_eq!(
        accessible(&slider),
        Accessible {
            role: Some(gpui_kit::Role::Slider),
            numeric: Some(30.),
            min: Some(0.),
            max: Some(100.),
            step: Some(5.),
            ..Default::default()
        }
    );
    let progress = wire::Node::Progress {
        key: "p".into(),
        value: 0.25,
        min: 0.,
        max: 1.,
        axis: wire::Axis::Row,
        length: None,
        girth: None,
        tone: None,
        background: None,
        bar: None,
        border: None,
    };
    assert_eq!(
        accessible(&progress),
        Accessible {
            role: Some(gpui_kit::Role::ProgressIndicator),
            numeric: Some(0.25),
            min: Some(0.),
            max: Some(1.),
            ..Default::default()
        }
    );
}

#[test]
fn an_input_is_named_by_its_label_and_a_secure_one_never_reports_its_value() {
    assert_eq!(
        accessible(&input("Room name", false, false)),
        Accessible {
            role: Some(gpui_kit::Role::TextInput),
            name: Some("Room name".into()),
            description: Some("Shown to members".into()),
            value: Some("hunter2".into()),
            ..Default::default()
        }
    );
    let secure = accessible(&input("Password", true, true));
    assert_eq!(secure.role, Some(gpui_kit::Role::PasswordInput));
    assert_eq!(secure.value, None);
    assert!(secure.disabled);
    // the placeholder is not a name
    assert_eq!(accessible(&input("", false, false)).name, None);
}

#[test]
fn an_editor_is_named_by_its_label_and_described_by_its_placeholder_without_one() {
    let mut editor = wire::Node::Editor {
        options: Box::default(),
        key: "e".into(),
        label: None,
        placeholder: "Write something".into(),
        document: wire::editor_document::EditorDocumentRef {
            document: "e".into(),
            reset: 1,
            text_revision: 0,
            revision: 0,
            cursor: Default::default(),
            byte_len: 0,
        },
        on_document: 0,
        editable: true,
        width: None,
        height: None,
        min_height: None,
        max_height: None,
    };
    assert_eq!(
        accessible(&editor),
        Accessible {
            role: Some(gpui_kit::Role::MultilineTextInput),
            description: Some("Write something".into()),
            ..Default::default()
        }
    );
    let wire::Node::Editor { label, .. } = &mut editor else {
        unreachable!()
    };
    *label = Some("Message".into());
    let named = accessible(&editor);
    assert_eq!(named.name.as_deref(), Some("Message"));
    // the placeholder is not a name, nor a second one
    assert_eq!(named.description, None);
}

/// #139: the field reads the text it holds, and one that takes no typing
/// says so and is offered none — the door reads the tree a screen reader
/// does.
#[gpui_kit::test]
fn a_read_only_editor_reads_its_text_and_is_offered_no_typing(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::init);
    let words = "no channel is open";
    for editable in [true, false] {
        let root = wire::Node::Editor {
            key: "composer".into(),
            label: Some("Message".into()),
            options: Box::default(),
            placeholder: String::new(),
            document: wire::editor_document::EditorDocumentRef {
                document: "composer".into(),
                reset: 1,
                text_revision: 0,
                revision: 0,
                cursor: Default::default(),
                byte_len: words.len() as u32,
            },
            on_document: 0,
            editable,
            width: None,
            height: None,
            min_height: None,
            max_height: None,
        };
        let store = crate::editor::wire::EditorStore::new(91);
        store.replace(&root).unwrap();
        seed_editor_text(&store, words);
        let window = cx.open_window(size(px(400.), px(300.)), |_, cx| {
            let mut tree = ViewTree::new(root);
            tree.set_editor_store(store, cx);
            tree
        });
        let mut native = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        let nodes = native.update(|window, cx| {
            window.activate_a11y();
            window.render_frame(cx);
            window.render_frame(cx);
            crate::ax_door::snapshot("t", window, false)
        });
        let field = nodes
            .iter()
            .find(|node| node.role == "MultilineTextInput")
            .expect("the editor's field is in the tree");
        assert_eq!(field.value.as_deref(), Some(words), "editable: {editable}");
        match editable {
            true => {
                assert!(!field.state.contains(&"disabled"));
                assert!(field.actions.contains(&"type"));
                assert!(field.actions.contains(&"set_value"));
            }
            false => {
                assert!(field.state.contains(&"disabled"));
                assert!(field.actions.is_empty(), "{:?}", field.actions);
            }
        }
    }
}

#[test]
fn a_picker_reports_the_chosen_option_as_its_value() {
    let options = vec!["Low".to_owned(), "High".to_owned()];
    let combo = |selected| wire::Node::ComboBox {
        key: "c".into(),
        label: None,
        state_key: "c".into(),
        options: options.clone(),
        selected,
        reset: 0,
        placeholder: "Priority".into(),
        on_select: 1,
        width: None,
        settings: Box::default(),
    };
    let pick = |selected| wire::Node::PickList {
        settings: Box::default(),
        key: "p".into(),
        label: None,
        options: options.clone(),
        selected,
        placeholder: Some("Priority".into()),
        on_select: 1,
        width: None,
        style: Default::default(),
    };
    for node in [combo(Some(1)), pick(Some(1))] {
        assert_eq!(
            accessible(&node),
            Accessible {
                role: Some(gpui_kit::Role::ComboBox),
                value: Some("High".into()),
                ..Default::default()
            }
        );
    }
    for node in [combo(None), pick(None), combo(Some(7)), pick(Some(7))] {
        assert_eq!(accessible(&node).value, None);
    }
    for mut node in [combo(Some(1)), pick(Some(1))] {
        assert_eq!(accessible(&node).name, None);
        let (wire::Node::ComboBox { label, .. } | wire::Node::PickList { label, .. }) = &mut node
        else {
            unreachable!()
        };
        *label = Some("Priority".into());
        assert_eq!(accessible(&node).name.as_deref(), Some("Priority"));
    }
}

#[test]
fn a_labelled_picture_is_an_image_and_an_unlabelled_one_is_decoration() {
    for node in picture(Some("Ada's avatar")) {
        assert_eq!(
            accessible(&node),
            Accessible {
                role: Some(gpui_kit::Role::Image),
                name: Some("Ada's avatar".into()),
                ..Default::default()
            }
        );
    }
    for node in picture(None).into_iter().chain(picture(Some(""))) {
        assert_eq!(accessible(&node), Accessible::default());
    }
}

#[test]
fn layout_is_not_in_the_accessibility_tree() {
    let space = wire::Node::Space {
        width: None,
        height: None,
    };
    assert_eq!(accessible(&space), Accessible::default());
}

#[test]
fn a_slider_is_named_by_its_label() {
    let slider = |label: Option<&str>| wire::Node::Slider {
        key: "s".into(),
        label: label.map(str::to_owned),
        value: 1.,
        min: 0.,
        max: 2.,
        step: 1.,
        on_change: 1,
        on_release: None,
        axis: wire::Axis::Row,
        width: None,
        height: None,
        style: Default::default(),
    };
    assert_eq!(
        accessible(&slider(Some("Volume"))).name.as_deref(),
        Some("Volume")
    );
    assert_eq!(accessible(&slider(Some(""))).name, None);
    assert_eq!(accessible(&slider(None)).name, None);
}

#[test]
fn a_text_heading_has_its_level_and_a_live_text_its_politeness() {
    let text = |heading, live| wire::Node::Text {
        id: Some(named_id("t")),
        style: gpui_kit::StyleRefinement::default(),
        content: "Members".into(),
        heading,
        live,
    };
    for level in 1..=6u8 {
        assert_eq!(
            accessible(&text(Some(level), None)),
            Accessible {
                role: Some(gpui_kit::Role::Heading),
                name: Some("Members".into()),
                level: Some(level.into()),
                ..Default::default()
            }
        );
    }
    for live in [wire::Live::Polite, wire::Live::Assertive] {
        assert_eq!(
            accessible(&text(None, Some(live))),
            Accessible {
                role: Some(gpui_kit::Role::Label),
                name: Some("Members".into()),
                live: Some(live),
                ..Default::default()
            }
        );
    }
}

/// Every role a view can give a mouse area or a button, and what
/// assistive technology hears for it.
const ROLES: [(wire::Role, gpui_kit::Role); 7] = [
    (wire::Role::Button, gpui_kit::Role::Button),
    (wire::Role::Link, gpui_kit::Role::Link),
    (wire::Role::Tab, gpui_kit::Role::Tab),
    (wire::Role::MenuItem, gpui_kit::Role::MenuItem),
    (wire::Role::Row, gpui_kit::Role::Row),
    (wire::Role::Checkbox, gpui_kit::Role::CheckBox),
    (wire::Role::Switch, gpui_kit::Role::Switch),
];

#[test]
fn a_button_with_a_role_is_that_role_and_reports_selected() {
    for (role, heard) in ROLES {
        let mut tab = button(wire::ButtonContent::Label("Tab".into()), None, Some(1));
        let wire::Node::Button {
            role: set,
            selected,
            ..
        } = &mut tab
        else {
            unreachable!()
        };
        (*set, *selected) = (Some(role), Some(true));
        let tab = accessible(&tab);
        assert_eq!(tab.role, Some(heard));
        assert_eq!(tab.selected, Some(true));
        assert_eq!(tab.name.as_deref(), Some("Tab"));
    }
}

#[test]
fn a_mouse_area_is_announced_only_as_the_role_its_view_gives_it() {
    let area = |role, label: Option<&str>| wire::Node::MouseArea {
        key: "m".into(),
        role,
        label: label.map(str::to_owned),
        expanded: Some(true),
        selected: Some(false),
        checked: Some(true),
        on_press: Some(1),
        on_release: None,
        on_double_click: None,
        on_right_press: None,
        on_right_release: None,
        on_middle_press: None,
        on_middle_release: None,
        on_enter: None,
        on_exit: None,
        on_move: None,
        on_press_at: None,
        on_scroll: None,
        content: Box::new(wire::Node::empty()),
    };
    for (role, heard) in ROLES {
        assert_eq!(
            accessible(&area(Some(role), Some("Wrap lines"))),
            Accessible {
                role: Some(heard),
                name: Some("Wrap lines".into()),
                toggled: Some(true),
                expanded: Some(true),
                selected: Some(false),
                ..Default::default()
            }
        );
        assert_eq!(accessible(&area(Some(role), Some(""))).name, None);
    }
    assert_eq!(
        accessible(&area(None, Some("Wrap lines"))),
        Accessible::default()
    );
}

#[test]
fn a_named_overlay_is_a_dialog_and_an_unnamed_one_is_layout() {
    let overlay = |label: Option<&str>, open| wire::Node::Overlay {
        key: "o".into(),
        label: label.map(str::to_owned),
        padding: 0.,
        backdrop: wire::Rgba([0.; 4]),
        align_x: wire::AlignX::Center,
        align_y: wire::AlignY::Center,
        on_dismiss: None,
        children: if open {
            vec![wire::Node::empty(), wire::Node::empty()]
        } else {
            Vec::new()
        },
    };
    assert_eq!(
        accessible(&overlay(Some("Rename channel"), true)),
        Accessible {
            role: Some(gpui_kit::Role::Dialog),
            name: Some("Rename channel".into()),
            ..Default::default()
        }
    );
    assert_eq!(
        accessible(&overlay(Some("Rename channel"), false)),
        Accessible::default()
    );
    assert_eq!(accessible(&overlay(None, true)), Accessible::default());
}
