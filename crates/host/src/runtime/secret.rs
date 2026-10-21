//! `store.secret_get` / `store.secret_set`: a view's secrets, kept by the OS's key store
//! (`contract::Secrets`) under a name of the network, the seated key, the program and the
//! view's key, never in the store's file.

use super::View;
use super::kernel::{Answer, in_order, spawn_reply};
use super::wire::{self, methods, methods::Capability, methods::refusal};
use crate::contract::{KeyRefusal, Secrets};

const NO_STORE: &str = "This device has no key store, so it keeps no secret.";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Request {
    Get(String),
    Set(String, Option<Vec<u8>>),
}

impl Request {
    fn read(
        capability: Capability,
        operation: &str,
        payload: &[u8],
    ) -> Option<Result<Self, wire::Error>> {
        let read = match (capability, operation) {
            (Capability::Store, "secret_get") => methods::decode(payload).map(Self::Get),
            (Capability::Store, "secret_set") => methods::decode(payload)
                .map(|(key, value): (String, Option<Vec<u8>>)| Self::Set(key, value)),
            _ => return None,
        };
        Some(read.map_err(malformed).and_then(Self::named))
    }

    fn key(&self) -> &str {
        match self {
            Self::Get(key) | Self::Set(key, _) => key,
        }
    }

    fn named(self) -> Result<Self, wire::Error> {
        match self.key().is_empty() {
            true => Err(malformed("a secret's key is not empty")),
            false => Ok(self),
        }
    }
}

pub(super) fn answer(
    view: &mut View,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
    props: &Option<Vec<u8>>,
) -> bool {
    let Some(read) = Request::read(capability, operation, payload) else {
        return false;
    };
    let asked = read.and_then(|request| Ok((name_of(view, props, request.key())?, request)));
    match asked {
        Ok((name, request)) => ask(view, id, name, request),
        Err(refused) => view.reply(id, Err(refused)),
    }
    true
}

fn name_of(view: &View, props: &Option<Vec<u8>>, key: &str) -> Result<String, wire::Error> {
    let connection = &view.connection;
    if connection.stale() {
        return Err(wire::Error::new(
            refusal::STALE_CONNECTION,
            "view belongs to a previous network connection",
        ));
    }
    if connection.chain.is_empty() {
        return Err(wire::Error::new(
            refusal::NOT_CONNECTED,
            "not connected to a network",
        ));
    }
    let signer = props
        .as_deref()
        .and_then(super::store::signer_of)
        .unwrap_or_default();
    if signer.is_empty() {
        return Err(wire::Error::new(
            refusal::SESSION_LOCKED,
            "no key is unlocked, and a secret is kept only for one",
        ));
    }
    Ok(name(&connection.chain, &signer, view.program, key))
}

fn name(chain: &str, signer: &str, program: &str, key: &str) -> String {
    let escaped = super::store::escaped;
    format!(
        "view-secret:{}:{}:{}:{}",
        escaped(chain),
        escaped(signer),
        escaped(program),
        escaped(key)
    )
}

fn ask(view: &mut View, id: u64, name: String, request: Request) {
    let (tell, answer) = tokio::sync::oneshot::channel();
    spawn_reply(view, id, async move {
        answer.await.unwrap_or_else(|_| {
            Err(wire::Error::new(
                refusal::HOST_FAULT,
                "the key store job died",
            ))
        })
    });
    let queue = name.clone();
    in_order(&queue, move || {
        let _ = tell.send(kept(crate::contract::os().secrets(), &name, &request));
    });
}

fn kept(secrets: Option<&dyn Secrets>, name: &str, request: &Request) -> Answer {
    let secrets = secrets
        .ok_or_else(|| KeyRefusal::NoStore(NO_STORE.into()))
        .map_err(refused)?;
    let answered = match request {
        Request::Get(_) => secrets.get(name).map(|value| methods::encode(&value)),
        Request::Set(_, value) => secrets
            .set(name, value.as_deref())
            .map(|()| methods::encode(&())),
    };
    answered.map_err(refused)
}

fn refused(why: KeyRefusal) -> wire::Error {
    let code = match why {
        KeyRefusal::NoStore(_) => refusal::NO_SECRET_STORE,
        KeyRefusal::Denied(_) | KeyRefusal::Broken(_) => refusal::SECRET_STORE_REFUSED,
    };
    wire::Error::new(code, why.sentence())
}

fn malformed(error: impl std::fmt::Display) -> wire::Error {
    wire::Error::new(refusal::MALFORMED_REQUEST, error.to_string())
}

#[cfg(test)]
mod tests;
