use super::*;

fn seated(signer: &str) -> Option<Vec<u8>> {
    Some(super::super::roster::props(
        true, "dev#01", signer, None, "",
    ))
}

fn connected() -> (View, std::sync::Arc<super::super::roster::Network>) {
    let mut view = crate::runtime::kernel::tests::view();
    let network = super::super::roster::Network::new(true);
    view.connection = super::super::Connection {
        chain: "dev#01".into(),
        rev: view.connection.rev.wrapping_sub(1),
        on: super::super::roster::On::of(&network),
        ..Default::default()
    };
    (view, network)
}

fn answers(view: &mut View, enough: usize) -> Vec<(u64, Answer)> {
    let mut changed = view.replies.changes();
    let mut heard = Vec::new();
    futures::executor::block_on(async {
        while heard.len() < enough {
            changed.borrow_and_update();
            view.replies.drain_into(&mut heard);
            if heard.len() < enough {
                changed.changed().await.expect("the key store replied");
            }
        }
    });
    let mut answered: Vec<(u64, Answer)> = heard
        .into_iter()
        .filter_map(|event| match event {
            wire::Event::Response { id, result, .. } => Some((id, result)),
            _ => None,
        })
        .collect();
    answered.sort_by_key(|(id, _)| *id);
    answered
}

#[test]
fn asks_read_as_their_requests_and_a_key_is_never_empty() {
    let key = methods::encode(&"dc/2".to_owned());
    let pair = methods::encode(&("dc/2".to_owned(), Some(b"k".to_vec())));
    assert_eq!(
        Request::read(Capability::Store, "secret_get", &key),
        Some(Ok(Request::Get("dc/2".into())))
    );
    assert_eq!(
        Request::read(Capability::Store, "secret_set", &pair),
        Some(Ok(Request::Set("dc/2".into(), Some(b"k".to_vec()))))
    );
    assert_eq!(Request::read(Capability::Store, "get", &key), None);
    let empty = methods::encode(&String::new());
    let refused = |operation: &str, payload: &[u8]| {
        Request::read(Capability::Store, operation, payload)
            .unwrap()
            .unwrap_err()
            .code
    };
    assert_eq!(refused("secret_get", &empty), refusal::MALFORMED_REQUEST);
    let unnamed = methods::encode(&(String::new(), Some(b"k".to_vec())));
    assert_eq!(refused("secret_set", &unnamed), refusal::MALFORMED_REQUEST);
    assert_eq!(refused("secret_set", &[1]), refusal::MALFORMED_REQUEST);
}

#[test]
fn a_secret_is_named_by_its_network_key_program_and_key_alone() {
    let named = name("dev#01", "aa01", "telegram", "dc/2");
    assert_eq!(named, "view-secret:dev_2301:aa01:telegram:dc_2f2");
    for other in [
        name("dev#02", "aa01", "telegram", "dc/2"),
        name("dev#01", "bb02", "telegram", "dc/2"),
        name("dev#01", "aa01", "chat", "dc/2"),
        name("dev#01", "aa01", "telegram", "dc/4"),
    ] {
        assert_ne!(other, named);
    }
    assert_ne!(name("a:b", "c", "p", "k"), name("a", "b:c", "p", "k"));
}

#[test]
fn a_secret_needs_a_seated_key() {
    let (view, _network) = connected();
    for props in [None, seated("")] {
        assert_eq!(
            name_of(&view, &props, "dc/2").unwrap_err().code,
            refusal::SESSION_LOCKED
        );
    }
    assert_eq!(
        name_of(&view, &seated("aa01"), "dc/2").unwrap(),
        name("dev#01", "aa01", view.program, "dc/2")
    );
}

#[test]
fn a_device_with_no_key_store_keeps_no_secret() {
    for request in [
        Request::Get("dc/2".into()),
        Request::Set("dc/2".into(), Some(b"k".to_vec())),
    ] {
        let refused = kept(None, "view-secret:dev_2301:aa01:telegram:dc_2f2", &request);
        assert_eq!(refused.unwrap_err().code, refusal::NO_SECRET_STORE);
    }
}

#[test]
fn a_secret_is_kept_by_the_key_store_read_back_and_removed() {
    let mut view = crate::runtime::kernel::tests::view();
    let key = "kept-read-removed";
    let named = name("dev#01", "aa01", "telegram", key);
    let set = |value: Option<&[u8]>| Request::Set(key.into(), value.map(<[u8]>::to_vec));
    ask(&mut view, 1, named.clone(), set(Some(b"auth key")));
    ask(&mut view, 2, named.clone(), Request::Get(key.into()));
    ask(&mut view, 3, named.clone(), set(None));
    ask(&mut view, 4, named.clone(), Request::Get(key.into()));
    let done = methods::encode(&());
    assert_eq!(
        answers(&mut view, 4),
        [
            (1, Ok(done.clone())),
            (2, Ok(methods::encode(&Some(b"auth key".to_vec())))),
            (3, Ok(done)),
            (4, Ok(methods::encode(&None::<Vec<u8>>))),
        ]
    );
    assert!(!crate::contract::testing::secrets_kept().contains_key(&named));
}

#[test]
fn a_key_store_that_says_no_is_told_to_the_view() {
    crate::contract::testing::refuse_secrets(
        "says-no",
        KeyRefusal::Denied("The key store is locked.".into()),
    );
    let mut view = crate::runtime::kernel::tests::view();
    let named = name("dev#01", "aa01", "telegram", "says-no");
    ask(&mut view, 1, named.clone(), Request::Get("says-no".into()));
    ask(
        &mut view,
        2,
        named.clone(),
        Request::Set("says-no".into(), Some(b"k".to_vec())),
    );
    for (_, answer) in answers(&mut view, 2) {
        assert_eq!(answer.unwrap_err().code, refusal::SECRET_STORE_REFUSED);
    }
    assert!(!crate::contract::testing::secrets_kept().contains_key(&named));
}
