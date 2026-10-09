//! Debug builds only: `ducktape-app --live <endpoint> <entry> <seed>`
//! opens one view in a window of its own on the node at `endpoint`, with
//! no launcher, desk or rail around it: a view developer's loop, driven
//! from the modules repository (`cargo run -p <view> --example live`,
//! which serves a fake node of the real programs and starts this).
//!
//! What the launcher does on a sign-in, done straight: the key seated is
//! `ed25519::from_seed(seed)` (the fake node holds the same key's account),
//! the node is read once for its status, the session and the account are
//! told, and the account is resolved through identity as always. The view
//! is `entry`, the roster entry its program (or its view-only listing) is
//! under. The window the view opens in is the one dialogs open in, so a
//! consent card has a place. The app quits with its last window.

use super::entities::Entities;
use commonware_cryptography::{Signer as _, ed25519};

pub(crate) struct Live {
    endpoint: String,
    module: &'static str,
    seed: u64,
}

impl Live {
    /// The arguments after `--live`, or the usage line and exit 2.
    pub(crate) fn parse() -> Live {
        let args: Vec<String> = std::env::args().skip(2).collect();
        let parsed = match args.as_slice() {
            [endpoint, module, seed] if program::is_name(module) => {
                seed.parse().ok().map(|seed| Live {
                    endpoint: endpoint.trim_end_matches('/').to_owned(),
                    module: crate::runtime::intern(module),
                    seed,
                })
            }
            _ => None,
        };
        parsed.unwrap_or_else(|| {
            eprintln!("usage: ducktape-app --live <http://host:port> <program-id> <seed>");
            std::process::exit(2)
        })
    }
}

/// The session on `live.endpoint` as `live.seed`'s key, and the view in
/// its window. A node that does not answer ends the process with its
/// reason: there is nothing to show without one.
pub(crate) fn boot(live: Live, entities: &Entities, cx: &mut gpui_kit::App) {
    let runtime = crate::runtime::handle();
    let signer = runtime.block_on(crate::backend::seat_key(ed25519::PrivateKey::from_seed(
        live.seed,
    )));
    let client = crate::backend::RpcClient::new(live.endpoint.clone());
    let status = match runtime.block_on(client.status()) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("{}: {error}", live.endpoint);
            std::process::exit(1);
        }
    };
    let chain = ducklink::ChainId::of(&status.network, &status.genesis)
        .map_or_else(|| status.network.clone(), |chain| chain.to_string());
    crate::runtime::connected(&client, &status.network, &chain);
    entities.session.update(cx, |session, cx| {
        session.live(live.endpoint.clone(), &status, chain, cx)
    });
    entities.account.update(cx, |account, cx| {
        account.live(signer, client, status.network.clone(), cx)
    });
    entities
        .windows
        .update(cx, |windows, cx| windows.open_live(live.module, cx));
    cx.on_window_closed(|cx, _| {
        cx.defer(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        });
    })
    .detach();
}
