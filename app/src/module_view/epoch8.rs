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
use duck_address::chat::MessageAddress;
use duck_address::forge::{ForgeLocator, ForgeRepoAddress, ForgeTarget};
use duck_address::identity::AccountAddress;
use duck_address::pages::PageAddress;
use duck_address::runs::RunAddress;
use duck_address::{Address, ChainId, number};
use files_wire::FileAddress;
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

/// An old-form link split as the app split it before the address move:
/// `duck://<module>/<path>[@<rev>][?net=<digest>][#<fragment>]`, every path
/// segment non-empty and neither `.` nor `..`, the query nothing or exactly
/// `net=` and the chain id's 8-hex half.
struct OldForm<'a> {
    module: &'a str,
    segments: Vec<&'a str>,
    rev: &'a str,
    net: &'a str,
    fragment: &'a str,
}

fn old_form(link: &str) -> Option<OldForm<'_>> {
    let rest = link.strip_prefix("duck://")?;
    let (body, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (address, query) = body.split_once('?').unwrap_or((body, ""));
    let net = match query {
        "" => "",
        query => query.strip_prefix("net=").filter(|net| hex(net, 8))?,
    };
    let (path, rev) = address.split_once('@').unwrap_or((address, ""));
    let (module, path) = path.split_once('/')?;
    let segments: Vec<&str> = path.split('/').collect();
    let clean = segments
        .iter()
        .all(|segment| !segment.is_empty() && *segment != "." && *segment != "..");
    clean.then_some(OldForm {
        module,
        segments,
        rev,
        net,
        fragment,
    })
}

fn hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A number the old form counted from 1.
fn positive(segment: &str) -> Option<u64> {
    number(segment).filter(|number| *number > 0)
}

/// The `base` an epoch-8 forge view hands `picture.inline`, read exactly as
/// the app read it before the address move: the one form that view mints,
/// `duck://forge/<repo>/blob/<path>[@<oid>][?net=<digest>]`, is the repo,
/// file and commit a README's relative pictures resolve against. Anything
/// else anchors nothing, as before.
pub(super) fn picture_base(base: &str) -> DuckLink {
    let read = || {
        let old = old_form(base).filter(|old| old.module == "forge" && old.fragment.is_empty())?;
        let oid = old.rev.is_empty() || hex(old.rev, 40);
        let [repo, "blob", file @ ..] = old.segments.as_slice() else {
            return None;
        };
        (oid && !file.is_empty()).then(|| DuckLink {
            repo: (*repo).to_owned(),
            path: file.join("/"),
            rev: old.rev.to_owned(),
            ..DuckLink::of(DuckKind::ForgeBlob)
        })
    };
    read().unwrap_or_else(|| DuckLink::of(DuckKind::Unknown))
}

/// The link an epoch-8 view hands `host.open_link`, as the open plane reads
/// it. An epoch-8 view mints links in the old form (`page`, `channel`,
/// `files`, `run`, `forge`, `account`); each is re-spelled as the address it
/// names on the chain the view was handed (its `chain` prop), and then
/// opens through the one grammar and its scope check like any address.
/// What has no address (a flat forge repo name, a file at a forge head, a
/// malformed link), and every link when the view has no chain, goes on as
/// it came and the open plane refuses the old form, as it does for every
/// other surface. An address passes as is.
pub(super) fn open_link(link: String, chain: &str) -> String {
    let address = chain
        .parse::<ChainId>()
        .ok()
        .and_then(|chain| address(&link, chain));
    address.map_or(link, |address| address.to_string())
}

/// The old form's module table, each row as its module's typed address.
fn address(link: &str, chain: ChainId) -> Option<Address> {
    let old = old_form(link)?;
    // `?net=` carried the chain id's hex half and nothing of its label: a
    // link naming another half is spelled on it under this chain's label, so
    // the scope check refuses it as another network
    let chain = match old.net {
        "" => chain,
        net => format!("{}#{net}", chain.label).parse().ok()?,
    };
    let plain = old.rev.is_empty() && old.fragment.is_empty();
    let address = match (old.module, old.segments.as_slice()) {
        ("page", [page]) if old.rev.is_empty() => PageAddress {
            page: (*page).to_owned(),
            block: (!old.fragment.is_empty()).then(|| old.fragment.to_owned()),
        }
        .address(chain),
        ("channel", [channel]) if old.rev.is_empty() => MessageAddress {
            channel: (*channel).to_owned(),
            seq: match old.fragment {
                "" => None,
                seq => Some(positive(seq)?),
            },
        }
        .address(chain),
        // the one files path the old form named
        ("files", ["shared", "attachments", _, _]) if plain => FileAddress {
            path: old
                .segments
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        }
        .address(chain),
        ("run", [digest]) if plain && hex(digest, 64) => RunAddress {
            digest: (*digest).to_owned(),
        }
        .address(chain),
        ("account", [account]) if plain => AccountAddress {
            account: positive(account)?,
        }
        .address(chain),
        ("forge", segments) => return forge(segments, old.rev, old.fragment, chain),
        _ => return None,
    };
    address.ok()
}

/// An old forge tail, `<repo>`, `<repo>/<n>[#<seq>]` or
/// `<repo>/blob/<path>@<rev>`, whose repo is named `<owner>/<repo>`. What
/// the old form read as a flat name has no address, and neither has a file
/// at the head (no `@<rev>`): the address names a commit.
fn forge(segments: &[&str], rev: &str, fragment: &str, chain: ChainId) -> Option<Address> {
    let flat = match segments {
        [_] | [_, "blob", _, ..] => true,
        [_, item] => positive(item).is_some(),
        _ => false,
    };
    let [owner, repo, target @ ..] = segments else {
        return None;
    };
    let repo = ForgeRepoAddress::from_name(&format!("{owner}/{repo}"))
        .ok()
        .filter(|_| !flat)?;
    let target = match target {
        [] if rev.is_empty() && fragment.is_empty() => return repo.address(chain).ok(),
        [number] if rev.is_empty() => {
            let number = positive(number)?;
            match fragment {
                "" => ForgeTarget::Item { number },
                seq => ForgeTarget::Comment {
                    number,
                    seq: positive(seq)?,
                },
            }
        }
        ["blob", path @ ..] if fragment.is_empty() => ForgeTarget::Blob {
            rev: rev.to_owned(),
            path: path.iter().map(|segment| (*segment).to_owned()).collect(),
        },
        _ => return None,
    };
    ForgeLocator { repo, target }.address(chain).ok()
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

    /// THE OLD ACCOUNT SPELLING, READ AT ONE DOOR. An epoch-8 chat view's
    /// mention hands over `duck://account/<n>`: from that view it opens the
    /// account on the chain the view was handed; from an epoch-10 view, and
    /// with no chain to spell it on, it reaches the open plane as it came
    /// and is refused with the old-form sentence.
    #[test]
    fn the_old_account_spelling_opens_only_from_an_epoch_8_view() {
        use crate::backend::{OLD_FORM, resolve_duck_link};
        const HERE: &str = "dognet#b5b6ea90";
        let opened = |epoch: Epoch, link: &str, chain: &str| {
            resolve_duck_link(epoch.open_link(link.into(), chain), HERE.into())
        };
        let eight = opened(Epoch::Eight, "duck://account/7", HERE);
        assert_eq!(
            (eight.kind, eight.account.as_str()),
            (DuckKind::Account, "7")
        );
        for (epoch, chain) in [(Epoch::Ten, HERE), (Epoch::Eight, "")] {
            let refused = opened(epoch, "duck://account/7", chain);
            assert_eq!(
                (refused.kind, refused.refusal.as_str()),
                (DuckKind::Unknown, OLD_FORM),
                "{epoch:?} on {chain:?}"
            );
        }
        for spelled in ["duck://account/0", "duck://account/07", "duck://account/+7"] {
            assert_eq!(
                opened(Epoch::Eight, spelled, HERE).kind,
                DuckKind::Unknown,
                "{spelled}"
            );
        }
        let address = "duck://dognet-b5b6ea90/chat/general";
        assert_eq!(Epoch::Eight.open_link(address.into(), HERE), address);
    }

    /// EVERY OLD FORM AN EPOCH-8 VIEW MINTS, READ AT ONE DOOR. Handed to
    /// `host.open_link` by an epoch-8 view, each opens exactly what its
    /// address opens; the same string from an epoch-10 view, pasted or
    /// launched (both reach the open plane as they came) is refused with the
    /// old-form sentence.
    #[test]
    fn an_old_form_link_from_an_epoch_8_view_opens_what_its_address_opens() {
        use crate::backend::{OLD_FORM, resolve_duck_link};
        const HERE: &str = "dognet#b5b6ea90";
        const AT: &str = "duck://dognet-b5b6ea90";
        let net = "?net=b5b6ea90";
        let dispatch = "ab".repeat(32);
        let rev = "1".repeat(40);
        for (old, new) in [
            ("duck://page/pg-1".to_owned(), format!("{AT}/pages/pg-1")),
            (
                format!("duck://page/pg-1{net}#blk-7"),
                format!("{AT}/pages/pg-1/block/blk-7"),
            ),
            (
                "duck://channel/general".into(),
                format!("{AT}/chat/general"),
            ),
            (
                format!("duck://channel/general{net}#42"),
                format!("{AT}/chat/general/42"),
            ),
            (
                "duck://files/shared/attachments/u1/보고서.pdf".into(),
                format!("{AT}/files/shared/attachments/u1/%EB%B3%B4%EA%B3%A0%EC%84%9C.pdf"),
            ),
            (
                format!("duck://run/{dispatch}{net}"),
                format!("{AT}/runs/{dispatch}"),
            ),
            (
                "duck://forge/ducks/core".into(),
                format!("{AT}/forge/ducks/core"),
            ),
            (
                format!("duck://forge/ducks/core/58{net}"),
                format!("{AT}/forge/ducks/core/58"),
            ),
            (
                "duck://forge/ducks/core/58#12".into(),
                format!("{AT}/forge/ducks/core/58/comment/12"),
            ),
            (
                format!("duck://forge/ducks/core/blob/docs/logo.png@{rev}{net}"),
                format!("{AT}/forge/ducks/core/blob/{rev}/docs/logo.png"),
            ),
            ("duck://account/7".into(), format!("{AT}/identity/7")),
        ] {
            let address = resolve_duck_link(new, HERE.into());
            assert!(
                !matches!(address.kind, DuckKind::Unknown),
                "{old}: {}",
                address.refusal
            );
            let eight = resolve_duck_link(Epoch::Eight.open_link(old.clone(), HERE), HERE.into());
            assert_eq!(eight, address, "{old}");
            let ten = resolve_duck_link(Epoch::Ten.open_link(old.clone(), HERE), HERE.into());
            let pasted = resolve_duck_link(old.clone(), HERE.into());
            for refused in [ten, pasted] {
                assert_eq!(
                    (refused.kind, refused.refusal.as_str()),
                    (DuckKind::Unknown, OLD_FORM),
                    "{old}"
                );
            }
        }
    }

    /// What the translation does not open: a `?net=` naming another network
    /// is refused as that network; what has no address, and every old form
    /// from a view with no chain, gets the old-form sentence.
    #[test]
    fn an_old_form_link_that_names_nothing_here_is_refused() {
        use crate::backend::{OLD_FORM, foreign_network_error, resolve_duck_link};
        const HERE: &str = "dognet#b5b6ea90";
        let opened = |link: &str, chain: &str| {
            resolve_duck_link(Epoch::Eight.open_link(link.into(), chain), HERE.into())
        };
        let theirs = opened("duck://forge/ducks/core/58?net=aaaaaaaa", HERE);
        assert_eq!(theirs.kind, DuckKind::ForeignNetwork);
        assert_eq!(
            foreign_network_error(&theirs, HERE.into()),
            "this link belongs to network dognet#aaaaaaaa — this app is on dognet#b5b6ea90"
        );
        let rev = "1".repeat(40);
        for untranslatable in [
            // a flat repo name has no address
            "duck://forge/core".to_owned(),
            "duck://forge/core/58".into(),
            "duck://forge/core/58#12".into(),
            format!("duck://forge/core/blob/README.md@{rev}"),
            // a file at the head names no commit
            "duck://forge/ducks/core/blob/README.md".into(),
            "duck://forge/ducks/core/blob/README.md@main".into(),
            "duck://forge/ducks/core/58#x".into(),
            "duck://page/pg-1@v2".into(),
            "duck://page/pg-1?net=nope".into(),
            "duck://channel/general#0".into(),
            "duck://files/etc/passwd".into(),
            "duck://files/shared/attachments/../x".into(),
            "duck://run/abc".into(),
            "duck://team.duck/index.html".into(),
            "duck://".into(),
        ] {
            let refused = opened(&untranslatable, HERE);
            assert_eq!(
                (refused.kind, refused.refusal.as_str()),
                (DuckKind::Unknown, OLD_FORM),
                "{untranslatable}"
            );
        }
        let chainless = opened("duck://page/pg-1", "");
        assert_eq!(
            (chainless.kind, chainless.refusal.as_str()),
            (DuckKind::Unknown, OLD_FORM)
        );
    }
}
