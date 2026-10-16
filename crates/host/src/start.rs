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
    let consent = crate::runtime::consent::changes_channel();
    let changes = crate::runtime::changes_channel();
    started(boot, consent, changes, cx)
}

/// [`start`] on the process's consent and roster wakes (`consent`,
/// `changes`), which a test owns one each of.
fn started(
    boot: Boot,
    consent: futures::channel::mpsc::UnboundedReceiver<()>,
    changes: futures::channel::mpsc::UnboundedReceiver<()>,
    cx: &mut App,
) -> (Host, Inbound) {
    contract::install(boot.os);
    contract::install_runner(boot.runner);
    crate::runtime::notify::on_open(boot.open);
    let center = crate::runtime::notify::center().clone();
    let roster = crate::runtime::roster().clone();
    crate::perf::mark("boot");
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

    /// What `start` installs and spawns: a banner click opens through the
    /// boot's opener, the lifecycle reaches the seats (`Backgrounded`), and
    /// the signals the host does not consume go on to the shell, in order.
    /// (On wakes of the test's own: the process's consent and roster wakes
    /// are two other tests'.)
    #[gpui_kit::test]
    fn start_opens_a_banner_click_and_hands_on_the_rest(cx: &mut gpui_kit::TestAppContext) {
        let boot = Boot {
            os: contract::os().clone(),
            runner: contract::runner().clone(),
            open: opened,
        };
        let (host, inbound) = cx.update(|cx| {
            gpui_kit::init(cx);
            let wakes = || futures::channel::mpsc::unbounded().1;
            started(boot, wakes(), wakes(), cx)
        });
        let backgrounded = |cx: &mut gpui_kit::TestAppContext| {
            cx.update(|cx| {
                cx.try_global::<crate::runtime::Backgrounded>()
                    .map(|phase| phase.0)
            })
        };
        inbound.send(Signal::Back);
        inbound.send(Signal::BannerClick(Click::Updates));
        inbound.send(Signal::Lifecycle(gpui_kit::AppLifecyclePhase::Background));
        inbound.send(Signal::MemoryWarning);
        cx.run_until_parked();
        assert_eq!(*OPENED.lock().unwrap(), vec!["updates"]);
        assert_eq!(backgrounded(cx), Some(true));
        assert!(matches!(host.signals.try_recv(), Ok(Signal::Back)));
        assert!(matches!(host.signals.try_recv(), Ok(Signal::MemoryWarning)));
        assert!(
            host.signals.try_recv().is_err(),
            "the click or the phase went to the shell too"
        );
        inbound.send(Signal::Lifecycle(gpui_kit::AppLifecyclePhase::Foreground));
        cx.run_until_parked();
        assert_eq!(backgrounded(cx), Some(false));
    }
}
