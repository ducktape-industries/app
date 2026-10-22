//! Share the app's log, from the Account tab: a copy of this launch's log and the one before it,
//! named for this phone and the minute, handed to the OS for the person to send where they choose.

use super::Root;
use ducktape_host::backend::app_log::Launches;
use ducktape_host::contract::{Files, KeepDone};
use gpui_kit::Context;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SHARES: AtomicU64 = AtomicU64::new(0);

impl Root {
    pub(super) fn share_log(&mut self, cx: &mut Context<Self>) {
        let os = ducktape_host::contract::os();
        let (Some(log), Some(files)) = (crate::os::app_log(), os.files()) else {
            self.notice("This build keeps no log to share.".to_owned(), cx);
            return;
        };
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let name = name(&crate::os::phone(), seconds);
        let cache = os.dirs().cache.clone();
        let toast = self.shared.toast.clone();
        cx.spawn(async move |_, cx| {
            let prepared = cx
                .background_executor()
                .spawn(async move { prepare(log, &cache, &name) })
                .await;
            let answer = match prepared {
                Ok(prepared) => {
                    let (told, heard) = futures::channel::oneshot::channel();
                    let done: KeepDone = Box::new(move |answer| {
                        if told.send(answer).is_err() {
                            tracing::debug!(target: "ducktape::phone", "the log's share was answered after the shell went");
                        }
                    });
                    hand(prepared, files, done);
                    heard.await.unwrap_or(Ok(false))
                }
                Err(error) => Err(error.to_string()),
            };
            if let Err(error) = answer {
                let said = format!("The log was not shared: {error}");
                cx.update(|cx| toast.update(cx, |toast, cx| toast.show(said, cx)));
            }
        })
        .detach();
    }
}

struct Prepared {
    folder: PathBuf,
    path: PathBuf,
}

fn prepare(log: &Launches, cache: &Path, name: &str) -> std::io::Result<Prepared> {
    let folder = cache.join(format!(
        "log-share-{}",
        SHARES.fetch_add(1, Ordering::Relaxed)
    ));
    match copied(log, &folder, name) {
        Ok(path) => Ok(Prepared { folder, path }),
        Err(error) => {
            removed(&folder);
            Err(error)
        }
    }
}

fn hand(prepared: Prepared, files: &dyn Files, done: KeepDone) {
    let Prepared { folder, path } = prepared;
    let unkeep = files.keep(
        &path,
        Box::new(move |answer| {
            removed(&folder);
            done(answer);
        }),
    );
    drop(unkeep);
}

fn copied(log: &Launches, folder: &Path, name: &str) -> std::io::Result<PathBuf> {
    if folder.exists() {
        std::fs::remove_dir_all(folder)?;
    }
    std::fs::create_dir_all(folder)?;
    let path = folder.join(name);
    log.copy_to(&path)?;
    Ok(path)
}

fn removed(folder: &Path) {
    if let Err(error) = std::fs::remove_dir_all(folder) {
        tracing::warn!(target: "ducktape::phone", %error, folder = %folder.display(), "a shared log's copy was not removed");
    }
}

fn name(phone: &str, seconds: u64) -> String {
    let day = ducktape_host::runtime::day(seconds);
    let minute = seconds % 86_400 / 60;
    format!(
        "ducktape-{}-{}-{:02}{:02}Z.log",
        slug(phone),
        slug(&day),
        minute / 60,
        minute % 60
    )
}

fn slug(words: &str) -> String {
    words
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests;
