//! The inbox runner: one task per connection that asks the inbox at the
//! RPC address for its status, logs in with the seated key, reads the
//! first page and then follows `live`, filling the centre and raising the
//! banners the device's policy allows. Started once the network is in
//! hand and the key is seated (`Notifications`), dropped on disconnect.
//!
//! An address without an inbox (404) ends the runner: the centre says so.
//! A refused login ends it too, shown, never retried in a loop. Anything
//! else is "unreachable since": the page read this session stands, nothing
//! banners, and the runner tries again with backoff until the inbox is
//! back, when `entries` fills the gap.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
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

/// The running follower; dropping it stops the task and empties the
/// centre's client.
pub(crate) struct Follower {
    task: tokio::task::JoinHandle<()>,
    center: CenterHandle,
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.task.abort();
        let mut center = self.center.lock();
        center.set_client(None);
        center.set_state(InboxState::Off);
    }
}

/// Follows the inbox under `rpc` for `chain` into `center`, as `label`
/// in the inbox's session list.
pub(crate) fn start(rpc: &RpcClient, chain: &str, label: &str, center: CenterHandle) -> Follower {
    let client = Arc::new(Client::new(rpc, chain, label));
    let task = crate::runtime::handle().spawn(run(client, center.clone()));
    Follower { task, center }
}

fn unreachable(center: &CenterHandle, error: &Error) {
    let mut center = center.lock();
    if !matches!(center.state, InboxState::Unreachable { .. }) {
        tracing::info!(target: "ducktape::app", %error, "the inbox is unreachable");
        center.set_state(InboxState::Unreachable {
            since: super::wall(),
        });
    }
}

async fn run(client: Arc<Client>, center: CenterHandle) {
    let mut backoff = FIRST_BACKOFF;
    loop {
        match connect(&client, &center).await {
            Ok(()) => backoff = FIRST_BACKOFF,
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

/// One connection: status, the mutes and the first page (logging in on
/// the way), then `live` until it ends. `Ok` when the socket closed.
async fn connect(client: &Arc<Client>, center: &CenterHandle) -> Result<(), Error> {
    let status = client.status().await?;
    let mutes = client.mutes().await?;
    let page = client.entries(None, PAGE).await?;
    {
        let mut center = center.lock();
        center.set_client(Some(client.clone()));
        center.set_stuck(status.stopped.or(status.stalled));
        center.mutes = mutes;
        center.replace(page.entries, page.unread);
        center.set_state(InboxState::Ready);
    }
    wake();
    let mut live = client.live().await?;
    while let Some(push) = live.next().await {
        let push = push?;
        let (banner, withdrawn, refill) = {
            let mut center = center.lock();
            match push {
                Push::Entry { entry } => {
                    let id = entry.id as u64;
                    let banner = center.arrived(&Settings::kept(), *entry, Instant::now());
                    (banner.map(|notice| (notice, id)), Vec::new(), false)
                }
                Push::Read { ids } => {
                    let (unknown, tags) = center.read_elsewhere(&ids);
                    (None, tags, unknown)
                }
                Push::Gone { ids } | Push::Hidden { ids } => (None, center.removed(&ids), false),
                Push::Prefs { mutes } => {
                    // mutes apply to stored rows: what `entries` lists moved
                    center.set_mutes(mutes);
                    (None, Vec::new(), true)
                }
            }
        };
        if let Some((notice, id)) = banner {
            // the entry's own banner opens its row; a "more" banner opens nothing
            let entry = (!notice.tag.starts_with("more/")).then_some(id);
            in_order("inbox", move || _ = platform::post(&notice, entry));
        }
        for tag in withdrawn.into_iter().filter(|tag| !tag.is_empty()) {
            in_order("inbox", move || platform::withdraw(&tag));
        }
        if refill {
            // an id this page did not have, or the mutes moved: the page
            // and the inbox's count are read again
            let page = client.entries(None, PAGE).await?;
            center.lock().replace(page.entries, page.unread);
        }
        wake();
    }
    Ok(())
}
