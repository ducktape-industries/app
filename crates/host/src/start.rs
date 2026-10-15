//! The host's start: the two stores, the entities every shell draws from,
//! and the one task that drains what the shell feeds in
//! (`contract::Inbound`). Each shell adds its windows over what it gets
//! back.

use crate::contract::{self, Boot, Inbound, Signal};
use crate::session::Shared;
use gpui_kit::{App, AsyncApp};

/// What a shell draws from after `start`: the entities, and the signals the
/// host does not consume itself (`Back`, `InsetsChanged`, `MemoryWarning`),
/// handed on for the shell's own windows to act on.
pub struct Host {
    pub shared: Shared,
    pub signals: async_channel::Receiver<Signal>,
}

/// Installs the shell's services, opens the stores (the notification
/// centre's log off disk, the roster the node's reads fill), makes the
/// entities, and spawns the drain: `Lifecycle` to the session (the status
/// poll stops in the background and resumes in front), `BannerClick` to
/// the centre (`notify::clicked`), the rest to `Host::signals`.
pub fn start(boot: Boot, cx: &mut App) -> (Host, Inbound) {
    contract::install(boot.os);
    let center = crate::runtime::notify::center().clone();
    let roster = crate::runtime::roster().clone();
    crate::perf::mark("boot");
    let changes = crate::runtime::changes_channel();
    let shared = Shared::new(roster, center, changes, cx);
    let (inbound, signals) = Inbound::channel();
    let (to_shell, from_host) = async_channel::unbounded();
    let session = shared.session.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(signal) = signals.recv().await {
            match signal {
                Signal::Lifecycle(phase) => {
                    if session
                        .update(cx, |session, cx| session.lifecycle(phase, cx))
                        .is_err()
                    {
                        break;
                    }
                }
                Signal::BannerClick(click) => crate::runtime::notify::clicked(click),
                other => {
                    let _ = to_shell.try_send(other);
                }
            }
        }
    })
    .detach();
    (
        Host {
            shared,
            signals: from_host,
        },
        inbound,
    )
}
