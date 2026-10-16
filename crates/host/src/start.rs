//! The host's start: the two stores, the entities every shell draws from,
//! and the one task that drains what the shell feeds in
//! (`contract::Inbound`). Each shell adds its windows over what it gets
//! back.

use crate::contract::{self, Boot, Inbound, Signal};
use crate::session::{Session, Shared};
use gpui_kit::{App, AsyncApp, WeakEntity};

/// What a shell draws from after `start`: the entities, the signals the
/// host does not consume itself (`Back`, `InsetsChanged`, `MemoryWarning`),
/// handed on for the shell's own windows to act on, and the consent
/// queue's wake.
pub struct Host {
    /// The entities every shell draws from.
    pub shared: Shared,
    /// The [`Signal`]s the host hands on.
    pub signals: async_channel::Receiver<Signal>,
    /// One item each time the consent queue moved (an ask queued, or its
    /// request let go of it): the shell shows `consent::front()` and
    /// answers it with `consent::answer`, or refuses every ask
    /// (`consent::refuse_all`) when it has nowhere to show one.
    pub consent: futures::channel::mpsc::UnboundedReceiver<()>,
}

/// Installs the shell's services, its view runner and its banner opener,
/// opens the stores
/// (the notification centre's log off disk, the roster the node's reads
/// fill), makes the entities, and spawns the drain: `Lifecycle` to the
/// session (the status poll stops in the background and resumes in
/// front), `BannerClick` to the centre (`notify::clicked`, which opens
/// through `Boot::open`), the rest to `Host::signals`.
pub fn start(boot: Boot, cx: &mut App) -> (Host, Inbound) {
    contract::install(boot.os);
    contract::install_runner(boot.runner);
    crate::runtime::notify::on_open(boot.open);
    let consent = crate::runtime::consent::changes_channel();
    let center = crate::runtime::notify::center().clone();
    let roster = crate::runtime::roster().clone();
    crate::perf::mark("boot");
    let changes = crate::runtime::changes_channel();
    let shared = Shared::new(roster, center, changes, cx);
    let (inbound, signals) = Inbound::channel();
    let (to_shell, from_host) = async_channel::unbounded();
    drain(signals, shared.session.downgrade(), to_shell, cx);
    (
        Host {
            shared,
            signals: from_host,
            consent,
        },
        inbound,
    )
}

/// The one task that drains `Inbound` on the GPUI thread, until the
/// session is gone.
fn drain(
    signals: async_channel::Receiver<Signal>,
    session: WeakEntity<Session>,
    to_shell: async_channel::Sender<Signal>,
    cx: &mut App,
) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(signal) = signals.recv().await {
            match signal {
                Signal::Lifecycle(phase) => {
                    // seats stop turning in the background and turn again
                    // in front (`Backgrounded`)
                    let backgrounded = matches!(phase, gpui_kit::AppLifecyclePhase::Background);
                    cx.update(|cx| cx.set_global(crate::runtime::Backgrounded(backgrounded)));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::notify::{Click, Opened};
    use std::sync::Mutex;

    static OPENED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

    fn opened(opened: Opened) {
        OPENED.lock().unwrap().push(match opened {
            Opened::Link(_) => "link",
            Opened::Updates => "updates",
        });
    }

    /// The drain opens a banner click through the shell's opener and hands
    /// the signals the host does not consume on to the shell, in order.
    /// (`start` itself is not called here: it installs the process's
    /// consent and roster wakes, which two other tests own.)
    #[gpui_kit::test]
    fn the_drain_opens_a_banner_click_and_hands_on_the_rest(cx: &mut gpui_kit::TestAppContext) {
        crate::runtime::notify::on_open(opened);
        let session = cx.update(crate::session::tests::session);
        let (inbound, signals) = Inbound::channel();
        let (to_shell, from_host) = async_channel::unbounded();
        cx.update(|cx| drain(signals, session.downgrade(), to_shell, cx));
        inbound.send(Signal::Back);
        inbound.send(Signal::BannerClick(Click::Updates));
        inbound.send(Signal::MemoryWarning);
        cx.run_until_parked();
        assert_eq!(*OPENED.lock().unwrap(), vec!["updates"]);
        assert!(matches!(from_host.try_recv(), Ok(Signal::Back)));
        assert!(matches!(from_host.try_recv(), Ok(Signal::MemoryWarning)));
        assert!(
            from_host.try_recv().is_err(),
            "the click went to the shell too"
        );
    }
}
