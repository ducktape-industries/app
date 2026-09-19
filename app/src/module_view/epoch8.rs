//! A view built at wire epoch 8 — every view deployed before epoch 10 —
//! read into the tree the presenter works on. Its frame is decoded with the
//! epoch-8 wire and mapped here, once, into the epoch-10 tree: the fields
//! epoch 10 added (a label on Editor, Slider, ComboBox, PickList and
//! Overlay; role, label, expanded, selected and checked on MouseArea; role
//! and selected on Button; heading and live on Text) are `None`, which is
//! what an epoch-10 view that does not set them sends. Nothing downstream
//! knows which epoch a tree came in.
//!
//! Only `Node` changed shape between the epochs, so everything around it
//! crosses by re-encoding: the same bytes read by the other crate.
//!
//! Deleted the wave after epoch-10 views are deployed on every network,
//! together with `Epoch::Eight` and the `view-wire-8` dependency.
use crate::DuckKind;
use crate::backend::DuckLink;
use serde::Serialize;
use serde::de::DeserializeOwned;
use view_wire as wire;
use view_wire_8 as old;

/// One tick's bytes from an epoch-8 guest, as an epoch-10 frame.
pub(super) fn frame(bytes: &[u8]) -> Result<wire::Frame, String> {
    let mut frame: old::Frame = old::decode(bytes)?;
    let root = frame.root.take().map(node).transpose()?;
    let patches = std::mem::take(&mut frame.patches)
        .into_iter()
        .map(patch)
        .collect::<Result<_, _>>()?;
    Ok(wire::Frame {
        root,
        patches,
        ..same(&frame)?
    })
}

/// A value whose shape did not change, read by the other crate.
fn same<A: Serialize, B: DeserializeOwned>(value: &A) -> Result<B, String> {
    wire::decode(&old::encode(value))
}

fn patch(patch: old::Patch) -> Result<wire::Patch, String> {
    Ok(match patch {
        old::Patch::Replace { path, node: n } => wire::Patch::Replace {
            path,
            node: node(n)?,
        },
        old::Patch::Props { path, node: n } => wire::Patch::Props {
            path,
            node: node(n)?,
        },
        old::Patch::Insert {
            path,
            index,
            node: n,
        } => wire::Patch::Insert {
            path,
            index,
            node: node(n)?,
        },
        old::Patch::Remove { path, index } => wire::Patch::Remove { path, index },
        old::Patch::Move { path, from, to } => wire::Patch::Move { path, from, to },
    })
}

/// One node: its children are taken out and mapped on their own, the
/// childless node is mapped, and the children go back into the same slots.
fn node(mut node: old::Node) -> Result<wire::Node, String> {
    let children: Vec<old::Node> = node
        .children_mut()
        .iter_mut()
        .map(|child| std::mem::replace(child, old::Node::empty()))
        .collect();
    let mut mapped = match node {
        old::Node::Text {
            options,
            key,
            content,
            size,
            color,
            font,
            width,
            align_x,
        } => wire::Node::Text {
            options: same(&options)?,
            key,
            content,
            size,
            color: same(&color)?,
            font: same(&font)?,
            width: same(&width)?,
            align_x: same(&align_x)?,
            heading: None,
            live: None,
        },
        old::Node::MouseArea {
            key,
            on_press,
            on_release,
            on_double_click,
            on_right_press,
            on_right_release,
            on_middle_press,
            on_middle_release,
            on_enter,
            on_exit,
            on_move,
            on_press_at,
            on_scroll,
            content,
        } => wire::Node::MouseArea {
            key,
            role: None,
            label: None,
            expanded: None,
            selected: None,
            checked: None,
            on_press,
            on_release,
            on_double_click,
            on_right_press,
            on_right_release,
            on_middle_press,
            on_middle_release,
            on_enter,
            on_exit,
            on_move,
            on_press_at,
            on_scroll,
            content: same(&content)?,
        },
        old::Node::Editor {
            options,
            key,
            placeholder,
            document,
            on_document,
            editable,
            width,
            height,
            min_height,
            max_height,
        } => wire::Node::Editor {
            options: same(&options)?,
            key,
            placeholder,
            label: None,
            document: same(&document)?,
            on_document,
            editable,
            width,
            height: same(&height)?,
            min_height,
            max_height,
        },
        old::Node::Button {
            key,
            content,
            label,
            checked,
            expanded,
            description,
            on_press,
            width,
            height,
            padding,
            style,
        } => wire::Node::Button {
            key,
            content: same(&content)?,
            label,
            role: None,
            checked,
            expanded,
            selected: None,
            description,
            on_press,
            width: same(&width)?,
            height: same(&height)?,
            padding: same(&padding)?,
            style: same(&style)?,
        },
        old::Node::Slider {
            key,
            value,
            min,
            max,
            step,
            on_change,
            on_release,
            axis,
            width,
            height,
            style,
        } => wire::Node::Slider {
            key,
            label: None,
            value,
            min,
            max,
            step,
            on_change,
            on_release,
            axis: same(&axis)?,
            width: same(&width)?,
            height: same(&height)?,
            style: same(&style)?,
        },
        old::Node::ComboBox {
            key,
            state_key,
            options,
            selected,
            reset,
            placeholder,
            on_select,
            width,
            settings,
        } => wire::Node::ComboBox {
            key,
            state_key,
            options,
            selected,
            reset,
            placeholder,
            label: None,
            on_select,
            width: same(&width)?,
            settings: same(&settings)?,
        },
        old::Node::PickList {
            settings,
            key,
            options,
            selected,
            placeholder,
            on_select,
            width,
            style,
        } => wire::Node::PickList {
            settings: same(&settings)?,
            key,
            options,
            selected,
            placeholder,
            label: None,
            on_select,
            width: same(&width)?,
            style: same(&style)?,
        },
        old::Node::Overlay {
            key,
            padding,
            backdrop,
            align_x,
            align_y,
            on_dismiss,
            children,
        } => wire::Node::Overlay {
            key,
            label: None,
            padding,
            backdrop: same(&backdrop)?,
            align_x: same(&align_x)?,
            align_y: same(&align_y)?,
            on_dismiss,
            children: same(&children)?,
        },
        // every other variant is the same at both epochs
        unchanged => same(&unchanged)?,
    };
    let slots = mapped.children_mut();
    if slots.len() != children.len() {
        return Err("an epoch-8 node changed its number of children".into());
    }
    for (slot, child) in slots.iter_mut().zip(children) {
        *slot = self::node(child)?;
    }
    Ok(mapped)
}

/// The `base` an epoch-8 forge view hands `picture.inline`, read exactly as
/// the app read it before the address move: the one form that view mints,
/// `duck://forge/<repo>/blob/<path>[@<oid>][?net=<digest>]`, is the repo,
/// file and commit a README's relative pictures resolve against. Anything
/// else anchors nothing, as before.
pub(super) fn picture_base(base: &str) -> DuckLink {
    let read = || {
        let rest = base.strip_prefix("duck://forge/")?;
        let (body, fragment) = rest.split_once('#').unwrap_or((rest, ""));
        let (address, query) = body.split_once('?').unwrap_or((body, ""));
        let net = query.is_empty()
            || query
                .strip_prefix("net=")
                .is_some_and(::chat::client::is_chain_digest);
        let (path, rev) = address.split_once('@').unwrap_or((address, ""));
        let segments: Vec<&str> = path.split('/').collect();
        let clean = segments
            .iter()
            .all(|segment| !segment.is_empty() && *segment != "." && *segment != "..");
        let oid = rev.is_empty()
            || (rev.len() == 40 && rev.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        let [repo, "blob", file @ ..] = segments.as_slice() else {
            return None;
        };
        let named = net && clean && oid && fragment.is_empty() && !file.is_empty();
        named.then(|| DuckLink {
            repo: (*repo).to_owned(),
            path: file.join("/"),
            rev: rev.to_owned(),
            ..DuckLink::of(DuckKind::ForgeBlob)
        })
    };
    read().unwrap_or_else(|| DuckLink::of(DuckKind::Unknown))
}

#[cfg(test)]
mod tests {
    use super::super::{Epoch, Failure, Guest, shape};
    use super::*;

    fn text(key: &str, content: &str) -> wire::Node {
        wire::kit::text(key, content)
    }

    /// One of every node epoch 10 changed, nested so children cross too,
    /// with every field epoch 10 added left `None`.
    fn tree() -> wire::Node {
        let area = wire::Node::MouseArea {
            key: "area".into(),
            role: None,
            label: None,
            expanded: None,
            selected: None,
            checked: None,
            on_press: Some(1),
            on_release: None,
            on_double_click: Some(2),
            on_right_press: None,
            on_right_release: None,
            on_middle_press: None,
            on_middle_release: None,
            on_enter: None,
            on_exit: None,
            on_move: None,
            on_press_at: None,
            on_scroll: Some(3),
            content: Box::new(text("inside", "Open")),
        };
        let controls = wire::kit::column(
            "controls",
            [
                wire::Node::Button {
                    key: "button".into(),
                    content: wire::ButtonContent::Child(Box::new(area)),
                    label: Some("Open".into()),
                    role: None,
                    checked: Some(false),
                    expanded: None,
                    selected: None,
                    description: Some("opens it".into()),
                    on_press: Some(4),
                    width: Some(wire::Length::Fill),
                    height: None,
                    padding: None,
                    style: Default::default(),
                },
                wire::Node::Editor {
                    options: Default::default(),
                    key: "editor".into(),
                    placeholder: "Write".into(),
                    label: None,
                    document: wire::editor_document::EditorDocumentRef {
                        document: "draft".into(),
                        reset: 1,
                        revision: 2,
                        text_revision: 3,
                        byte_len: 0,
                        cursor: Default::default(),
                    },
                    on_document: 5,
                    editable: true,
                    width: Some(200.),
                    height: None,
                    min_height: Some(20.),
                    max_height: None,
                },
                wire::Node::Slider {
                    key: "slider".into(),
                    label: None,
                    value: 0.5,
                    min: 0.,
                    max: 1.,
                    step: 0.1,
                    on_change: 6,
                    on_release: Some(7),
                    axis: wire::Axis::Row,
                    width: None,
                    height: None,
                    style: Default::default(),
                },
                wire::Node::ComboBox {
                    key: "combo".into(),
                    state_key: "combo".into(),
                    options: vec!["Serif".into(), "Mono".into()],
                    selected: Some(1),
                    reset: 0,
                    placeholder: "Font".into(),
                    label: None,
                    on_select: 8,
                    width: None,
                    settings: Default::default(),
                },
                wire::Node::PickList {
                    settings: Default::default(),
                    key: "pick".into(),
                    options: vec!["Dark".into()],
                    selected: None,
                    placeholder: Some("Theme".into()),
                    label: None,
                    on_select: 9,
                    width: None,
                    style: Default::default(),
                },
            ],
        );
        wire::Node::Overlay {
            key: "overlay".into(),
            label: None,
            padding: 8.,
            backdrop: wire::Rgba([0., 0., 0., 0.5]),
            align_x: wire::AlignX::Center,
            align_y: wire::AlignY::Top,
            on_dismiss: Some(10),
            children: vec![controls, text("modal", "Saved")],
        }
    }

    /// What an epoch-8 build of the same view writes: the same value, its
    /// epoch-10 fields dropped (they are all `None` here).
    fn at_epoch_8<A: Serialize, B: DeserializeOwned>(value: &A) -> B {
        serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
    }

    #[test]
    fn an_epoch_8_frame_and_an_epoch_10_frame_of_one_tree_present_the_same_tree() {
        let frame = wire::Frame {
            root: Some(tree()),
            requests: vec![],
            cancels: vec![4],
            busy: true,
            ..Default::default()
        };
        let old_frame: old::Frame = at_epoch_8(&frame);
        let eight = old::encode(&old_frame);
        let ten = wire::encode(&frame);
        assert_ne!(eight, ten, "the fixture holds nodes epoch 10 changed");
        assert_eq!(Epoch::Eight.frame(&eight).unwrap(), frame);
        assert_eq!(Epoch::Ten.frame(&ten).unwrap(), frame);
        // and the host takes both the same way, sanitized
        let (eight, _) = shape(Epoch::Eight, &eight).unwrap();
        let (ten, _) = shape(Epoch::Ten, &ten).unwrap();
        assert_eq!(eight, ten);
        // the wrong epoch's reader does not quietly take the other's bytes
        assert_ne!(Epoch::Ten.frame(&old::encode(&old_frame)).ok(), Some(frame));
    }

    #[test]
    fn an_epoch_8_frame_of_patches_maps_every_patch() {
        let patches = vec![
            wire::Patch::Replace {
                path: vec![0],
                node: tree(),
            },
            wire::Patch::Props {
                path: vec![1],
                node: text("modal", "Saved again"),
            },
            wire::Patch::Insert {
                path: vec![0],
                index: 2,
                node: tree(),
            },
            wire::Patch::Remove {
                path: vec![0],
                index: 1,
            },
            wire::Patch::Move {
                path: vec![0],
                from: 0,
                to: 1,
            },
        ];
        let frame = wire::Frame {
            patches,
            ..Default::default()
        };
        let old_frame: old::Frame = at_epoch_8(&frame);
        assert_eq!(Epoch::Eight.frame(&old::encode(&old_frame)).unwrap(), frame);
    }

    /// Host → guest: what the host writes an epoch-8 guest is what that
    /// guest's own wire reads, byte for byte.
    #[test]
    fn an_epoch_8_guest_reads_the_events_the_host_writes_it() {
        let events = vec![
            wire::Event::Message(1),
            wire::Event::Input {
                handler: 2,
                text: "draft".into(),
            },
            wire::Event::Toggle {
                handler: 3,
                on: true,
            },
            wire::Event::Slide {
                handler: 4,
                value: 0.25,
            },
            wire::Event::Select {
                handler: 5,
                index: 1,
            },
            wire::Event::Pointer {
                handler: 6,
                x: 1.,
                y: 2.,
            },
            wire::Event::Drag {
                handler: 7,
                dx: 3.,
                dy: 4.,
            },
            wire::Event::Response {
                id: 8,
                result: Ok(wire::encode(&true)),
                done: true,
            },
            wire::Event::Response {
                id: 9,
                result: Err(wire::Refusal::new(
                    "stale_connection",
                    "network connection changed",
                )),
                done: true,
            },
            wire::Event::Resync,
        ];
        let written = Epoch::Eight.events(&events);
        assert_eq!(written, Epoch::Ten.events(&events));
        let read: Vec<old::Event> = old::decode(&written).unwrap();
        assert_eq!(read.len(), events.len());
        assert_eq!(old::encode(&read), written);
    }

    #[test]
    fn a_view_at_any_other_epoch_is_refused_by_name() {
        assert_eq!(Epoch::of(8), Ok(Epoch::Eight));
        assert_eq!(Epoch::of(10), Ok(Epoch::Ten));
        // a component whose manifest says epoch 11: refused before compiling
        let text = b"ducktape.view.manifest.v1\nSized\n\n\nnone\n11";
        let name = wire::manifest::MANIFEST_SECTION.as_bytes();
        let mut bytes = b"\0asm\x0d\0\x01\0".to_vec();
        bytes.extend([0, (1 + name.len() + text.len()) as u8, name.len() as u8]);
        bytes.extend(name);
        bytes.extend(text);
        let Err(failure) = Guest::compile(&bytes, "chat view") else {
            panic!("an epoch-11 view compiled");
        };
        assert_eq!(
            failure,
            Failure::WireEpoch(
                "chat view: this view speaks wire epoch 11; this app speaks 8 and 10".into()
            )
        );
        assert_eq!(failure.title(), "This view speaks a wire this app does not");
    }

    /// `picture.inline`'s base, per epoch: an epoch-8 forge view's spelling
    /// anchors the README as it always did, so a relative picture is read at
    /// the README's own commit beside it; an epoch-10 view's address anchors
    /// the same. Neither epoch reads the other's spelling.
    #[test]
    fn an_epoch_8_readme_anchors_its_relative_pictures_as_before() {
        use crate::backend::resolve_repo_path;
        let rev = "0123456789abcdef0123456789abcdef01234567";
        let anchored = |link: DuckLink| {
            let picture = resolve_repo_path(&link.path, "img/duck.png");
            (link.kind, link.repo, link.rev, picture)
        };
        let old = format!("duck://forge/core/blob/docs/README.md@{rev}?net=b5b6ea90");
        let eight = Epoch::Eight.picture_base(old.clone(), String::new());
        assert_eq!(
            anchored(eight),
            (
                DuckKind::ForgeBlob,
                "core".into(),
                rev.into(),
                Some("docs/img/duck.png".into())
            )
        );
        let unpinned =
            Epoch::Eight.picture_base("duck://forge/core/blob/README.md".into(), "".into());
        assert_eq!(
            (unpinned.repo.as_str(), unpinned.rev.as_str()),
            ("core", "")
        );
        for refused in [
            "duck://forge/core/blob/README.md@main",
            "duck://forge/core/blob/README.md?net=nope",
            "duck://forge/core/blob/../README.md",
            "duck://forge/core/7",
        ] {
            let link = Epoch::Eight.picture_base(refused.into(), "".into());
            assert_eq!(
                (link.kind, link.repo.as_str()),
                (DuckKind::Unknown, ""),
                "{refused}"
            );
        }

        let new = format!("duck://dognet-b5b6ea90/forge/ducks/core/blob/{rev}/docs/README.md");
        let ten = Epoch::Ten.picture_base(new.clone(), "dognet#b5b6ea90".into());
        assert_eq!(
            anchored(ten),
            (
                DuckKind::ForgeBlob,
                "ducks/core".into(),
                rev.into(),
                Some("docs/img/duck.png".into())
            )
        );
        let old_at_ten = Epoch::Ten.picture_base(old, "dognet#b5b6ea90".into());
        assert_eq!(old_at_ten.repo, "");
        let new_at_eight = Epoch::Eight.picture_base(new, "".into());
        assert_eq!(new_at_eight.repo, "");
    }

    /// The files route, per epoch: the app holds the full address; an
    /// epoch-10 files view is handed it as is, an epoch-8 one the bare
    /// duckfs path it has always been routed by. No other view's props move.
    #[test]
    fn the_files_route_is_the_address_at_ten_and_the_path_at_eight() {
        let address = "duck://dognet-b5b6ea90/files/shared/%EB%B3%B4%EA%B3%A0%EC%84%9C.pdf";
        let props = serde_json::json!({
            "chain": "dognet#b5b6ea90", "route": address, "route_serial": 2,
        });
        let bytes = serde_json::to_vec(&props).unwrap();
        let read = |bytes: Vec<u8>| serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
        assert_eq!(read(Epoch::Ten.props("files", bytes.clone())), props);
        let eight = read(Epoch::Eight.props("files", bytes.clone()));
        assert_eq!(eight["route"], "/shared/보고서.pdf");
        assert_eq!(eight["route_serial"], 2);
        assert_eq!(Epoch::Eight.props("forge", bytes.clone()), bytes);
        let empty = serde_json::to_vec(&serde_json::json!({ "route": "" })).unwrap();
        assert_eq!(read(Epoch::Eight.props("files", empty))["route"], "");
    }
}
