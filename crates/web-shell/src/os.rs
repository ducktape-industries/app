//! The browser's services for the read-only preview: kept files in memory for the tab's life, a throwaway key that signs
//! queries, no device key, no secret store and no banners.

use ducktape_host::contract::{
    Banners, Dirs, KeyRefusal, KeyStorage, Limits, MintInvite, OsFacts, OsServices, Preload,
    Secrets,
};
use ducktape_host::runtime::Answered;
use ducktape_host::runtime::notify::{Click, Notice};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

const NO_SIGN_IN: &str = "Signing in isn't in the web preview yet.";

pub struct WebOs {
    dirs: Dirs,
    disk: TabDisk,
    reader: Arc<dyn noded_client::Signer>,
}

impl WebOs {
    pub fn new() -> Self {
        let root = PathBuf::from("/web");
        Self {
            dirs: Dirs {
                cache: root.join("cache"),
                config: root.join("config"),
                data: root.join("data"),
                remotes: root.join("remotes"),
                runtime: root.join("runtime"),
                views_override: None,
            },
            disk: TabDisk::default(),
            reader: Arc::new(reader_key()),
        }
    }
}

fn reader_key() -> commonware_cryptography::ed25519::PrivateKey {
    use commonware_codec::DecodeExt as _;
    use rand::RngCore as _;
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);
    commonware_cryptography::ed25519::PrivateKey::decode(seed.as_slice())
        .expect("32 random bytes decode")
}

#[derive(Default)]
struct TabDisk(Mutex<HashMap<PathBuf, Vec<u8>>>);

impl TabDisk {
    fn files(&self) -> MutexGuard<'_, HashMap<PathBuf, Vec<u8>>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl noded_client::fs::Disk for TabDisk {
    fn read(&self, path: &Path) -> std::io::Result<Option<Vec<u8>>> {
        Ok(self.files().get(path).cloned())
    }

    fn write(&self, path: &Path, bytes: &[u8], _private: bool) -> std::io::Result<()> {
        self.files().insert(path.to_owned(), bytes.to_vec());
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        let mut files = self.files();
        let moved = files
            .remove(from)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
        files.insert(to.to_owned(), moved);
        Ok(())
    }
}

struct NoBanners;

impl Banners for NoBanners {
    fn post(&self, _: &Notice, _: Option<Click>) -> bool {
        false
    }

    fn withdraw(&self, _: &str, _: &str) {}
}

fn runs_on_a_mac() -> bool {
    crate::node::page()
        .navigator()
        .platform()
        .is_ok_and(|platform| platform.starts_with("Mac"))
}

impl OsServices for WebOs {
    fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    fn disk(&self) -> &dyn noded_client::fs::Disk {
        &self.disk
    }

    fn device_key(
        &self,
        _keyring: &str,
    ) -> rt::Fut<'static, Result<Option<Arc<dyn noded_client::Signer>>, KeyRefusal>> {
        Box::pin(std::future::ready(Err(KeyRefusal::NoStore(
            NO_SIGN_IN.into(),
        ))))
    }

    fn key_storage(&self, _keyring: &str) -> Result<KeyStorage, KeyRefusal> {
        Err(KeyRefusal::NoStore(NO_SIGN_IN.into()))
    }

    fn secrets(&self) -> Option<&dyn Secrets> {
        None
    }

    fn reader(&self) -> Arc<dyn noded_client::Signer> {
        self.reader.clone()
    }

    fn banners(&self) -> &dyn Banners {
        &NoBanners
    }

    fn facts(&self) -> OsFacts {
        OsFacts {
            mac_modifiers: runs_on_a_mac(),
            rss_unit: 1,
            touch_first: false,
            device_label: "Browser",
            device_noun: "browser",
            one_layout: false,
            floats: false,
            motion: ducktape_host::motion::IOS,
            os_asks_notices: false,
            feeds_follow_screen: false,
        }
    }

    fn limits(&self) -> Limits {
        Limits {
            loaders: 1,
            preload: Preload::Compile,
            snapshot_every: None,
            snapshot_to_disk: false,
        }
    }

    fn bundled_view(&self, _: &abi::BlobId) -> Option<Vec<u8>> {
        None
    }

    fn mint_invite(&self, _: MintInvite) -> Option<Answered> {
        None
    }
}
