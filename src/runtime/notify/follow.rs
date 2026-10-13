//! The inbox runner: one task per connection that asks the inbox at the
//! RPC address for its status, logs in with the seated key, opens `live`,
//! reads the first page and then follows the socket, filling the centre
//! and raising the banners the device's policy allows. Started once the
//! network is in hand and the key is seated (`Notifications`), dropped on
//! disconnect, when the key is locked, and at quit, with a logout.
//!
//! An address without an inbox (404) ends the runner: the centre says so.
//! A refused login ends it too, shown, never retried in a loop. Anything
//! else is "unreachable since": the page read this session stands, nothing
//! banners, and the runner tries again with backoff until the inbox is
//! back, `status` first, when `entries` fills the gap. The socket is opened
//! before the page is read, so no push falls between them; it is pinged
//! when quiet, so a dead path (a laptop woke, a NAT forgot it) is noticed
//! within two intervals and the connection is made again.

use std::sync::Arc;
use std::time::{Duration, Instant};

use inbox_api::Push;

use super::center::{CenterHandle, InboxState};
use super::settings::Settings;
use super::{platform, wake};
use crate::backend::RpcClient;
use crate::backend::inbox::{Client, Error};
use crate::runtime::kernel::in_order;

/// The first page the centre shows; older rows are the inbox's to keep.
const PAGE: u32 = 50;
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// The program's name as the shell shows it (a "N more from X" banner).
pub(crate) type LabelOf = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The running follower; dropping it stops the task, forgets the
/// account's page in the centre, and logs the session out.
pub(crate) struct Follower {
    task: tokio::task::JoinHandle<()>,
    center: CenterHandle,
    client: Arc<Client>,
}

impl Follower {
    /// Whether the task ended on its own (no inbox, a refused login): a
    /// later connect starts a new one.
    pub(crate) fn ended(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.task.abort();
        {
            let mut center = self.center.lock();
            // a write the task still makes is refused from here on
            center.claim();
            center.set_client(None);
            center.forget_account();
        }
        let client = self.client.clone();
        crate::runtime::handle().spawn(async move {
            if let Err(error) = client.logout().await {
                tracing::info!(target: "ducktape::app", kind = %error.kind(), "the inbox session was not logged out");
            }
        });
    }
}

/// Follows the inbox under `rpc` for `chain` into `center`, as `label`
/// in the inbox's session list.
pub(crate) fn start(rpc: &RpcClient, chain: &str, label: &str, center: CenterHandle) -> Follower {
    let client = Arc::new(Client::new(rpc, chain, label));
    start_with(client, center, Arc::new(rail_label))
}

/// The program as the rail names it: its view's title, else its id.
fn rail_label(program: &str) -> String {
    crate::runtime::roster()
        .rail()
        .into_iter()
        .find(|row| row.module == program)
        .map_or_else(|| program.to_owned(), |row| row.label)
}

pub(crate) fn start_with(client: Arc<Client>, center: CenterHandle, label_of: LabelOf) -> Follower {
    let stamp = center.lock().claim();
    let task = crate::runtime::handle().spawn(run(client.clone(), center.clone(), stamp, label_of));
    Follower {
        task,
        center,
        client,
    }
}

fn unreachable(center: &CenterHandle, error: &Error) {
    let mut center = center.lock();
    if !matches!(center.state, InboxState::Unreachable { .. }) {
        tracing::info!(target: "ducktape::app", kind = %error.kind(), "the inbox is unreachable");
        center.set_state(InboxState::Unreachable {
            since: super::wall(),
        });
    }
}

async fn run(client: Arc<Client>, center: CenterHandle, stamp: u64, label_of: LabelOf) {
    let mut backoff = FIRST_BACKOFF;
    loop {
        let connected = connect(&client, &center, stamp, &label_of, &mut backoff).await;
        if !center.lock().stamped(stamp) {
            return;
        }
        match connected {
            Ok(()) => {}
            Err(Error::NoInbox) => {
                center.lock().set_state(InboxState::NoInbox);
                wake();
                return;
            }
            Err(Error::Denied(why)) => {
                center.lock().set_state(InboxState::Denied(why));
                wake();
                return;
            }
            Err(error) => unreachable(&center, &error),
        }
        wake();
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// One connection: status, `live` (logging in on the way), the mutes and
/// the first page, then the socket until it ends. `Ok` when it closed.
async fn connect(
    client: &Arc<Client>,
    center: &CenterHandle,
    stamp: u64,
    label_of: &LabelOf,
    backoff: &mut Duration,
) -> Result<(), Error> {
    let status = client.status().await?;
    let mut live = client.live().await?;
    let mutes = client.mutes().await?;
    let page = client.entries(None, PAGE).await?;
    {
        let mut center = center.lock();
        if !center.stamped(stamp) {
            return Ok(());
        }
        center.set_client(Some(client.clone()));
        center.set_stuck(status.stopped.or(status.stalled));
        center.mutes = mutes;
        center.replace(page.entries, page.unread);
        center.set_state(InboxState::Ready);
        center.resend_watermarks();
    }
    *backoff = FIRST_BACKOFF;
    wake();
    while let Some(push) = live.next().await? {
        // the settings and the label first: neither under the centre
        // (lock order: a seat, then the centre; the prefs file off it)
        let settings = Settings::kept();
        let label = match &push {
            Push::Entry { entry } => label_of(&entry.program),
            _ => String::new(),
        };
        let (banner, refill) = {
            let mut center = center.lock();
            if !center.stamped(stamp) {
                return Ok(());
            }
            match push {
                Push::Entry { entry } => (
                    center.arrived(&settings, *entry, Instant::now(), &label),
                    false,
                ),
                Push::Read { ids } => (None, center.read_elsewhere(&ids)),
                Push::Gone { ids } | Push::Hidden { ids } => {
                    center.removed(&ids);
                    (None, false)
                }
                Push::Prefs { mutes } => {
                    // mutes apply to stored rows: what `entries` lists moved
                    center.set_mutes(mutes);
                    (None, true)
                }
                Push::Status(status) => {
                    center.set_stuck(status.stopped.or(status.stalled));
                    (None, false)
                }
            }
        };
        if let Some(notice) = banner {
            let entry = notice.opens.map(|id| id as u64);
            in_order("inbox", move || _ = platform::post(&notice, entry));
        }
        if refill {
            // an id this page did not have, or the mutes moved: the page
            // and the inbox's count are read again
            let page = client.entries(None, PAGE).await?;
            let mut center = center.lock();
            if !center.stamped(stamp) {
                return Ok(());
            }
            center.replace(page.entries, page.unread);
        }
        wake();
    }
    Ok(())
}
