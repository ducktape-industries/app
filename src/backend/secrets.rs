//! A view's secrets on the desktop (`contract::Secrets`): one item each in the OS key store
//! beside this device's key, or, with `DUCKTAPE_DEVICE_KEY_STORE=file`, one file each that
//! only this user can read.

use std::path::{Path, PathBuf};

use ducktape_host::contract::{KeyRefusal, Secrets};

const SERVICE: &str = "dev.ducktape.app";

pub(crate) struct DesktopSecrets;

impl Secrets for DesktopSecrets {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, KeyRefusal> {
        if super::device_key::file_only() {
            return read(&folder()?, name);
        }
        match entry(name)?.get_secret() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(refused(error)),
        }
    }

    fn set(&self, name: &str, value: Option<&[u8]>) -> Result<(), KeyRefusal> {
        if super::device_key::file_only() {
            return write(&folder()?, name, value);
        }
        let entry = entry(name)?;
        let done = match value {
            Some(value) => entry.set_secret(value),
            None => match entry.delete_credential() {
                Err(keyring::Error::NoEntry) => Ok(()),
                removed => removed,
            },
        };
        done.map_err(refused)
    }
}

fn entry(name: &str) -> Result<keyring::Entry, KeyRefusal> {
    let home = ducktape_home::root().map_err(KeyRefusal::NoStore)?;
    keyring::Entry::new(SERVICE, &format!("{}:{name}", home.display())).map_err(refused)
}

fn refused(error: keyring::Error) -> KeyRefusal {
    let no_store = matches!(error, keyring::Error::PlatformFailure(_));
    let sentence = super::device_key::refused(error);
    match no_store {
        true => KeyRefusal::NoStore(sentence),
        false => KeyRefusal::Denied(sentence),
    }
}

fn folder() -> Result<PathBuf, KeyRefusal> {
    ducktape_home::root()
        .map(|home| home.join("secrets"))
        .map_err(KeyRefusal::NoStore)
}

fn file(folder: &Path, name: &str) -> PathBuf {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(name.as_bytes());
    folder.join(
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
}

fn read(folder: &Path, name: &str) -> Result<Option<Vec<u8>>, KeyRefusal> {
    match std::fs::read(file(folder, name)) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(KeyRefusal::Denied(format!(
            "This secret could not be read: {error}"
        ))),
    }
}

fn write(folder: &Path, name: &str, value: Option<&[u8]>) -> Result<(), KeyRefusal> {
    let path = file(folder, name);
    let done = match value {
        Some(value) => crate::os::private_dir(folder)
            .and_then(|()| noded_client::fs::atomic_write(&path, value, true)),
        None => match std::fs::remove_file(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            removed => removed,
        },
    };
    done.map_err(|error| KeyRefusal::Denied(format!("This secret could not be kept: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_secret_is_kept_read_and_removed_where_only_this_user_reads_it() {
        let folder =
            std::env::temp_dir().join(format!("ducktape-secrets-{}", rand::random::<u64>()));
        let name = "view-secret:dev_2301:aa01:telegram:dc_2f2";
        let other = "view-secret:dev_2301:bb02:telegram:dc_2f2";
        assert_eq!(read(&folder, name), Ok(None));
        write(&folder, name, Some(b"auth key")).unwrap();
        assert_eq!(read(&folder, name), Ok(Some(b"auth key".to_vec())));
        assert_eq!(read(&folder, other), Ok(None));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&folder), 0o700);
            assert_eq!(mode(&file(&folder, name)) & 0o077, 0);
        }
        write(&folder, name, None).unwrap();
        write(&folder, name, None).unwrap();
        assert_eq!(read(&folder, name), Ok(None));
        let long = format!("view-secret:dev_2301:aa01:telegram:{}", "_2f".repeat(400));
        write(&folder, &long, Some(b"auth key")).unwrap();
        assert_eq!(read(&folder, &long), Ok(Some(b"auth key".to_vec())));
        let _ = std::fs::remove_dir_all(folder);
    }
}
