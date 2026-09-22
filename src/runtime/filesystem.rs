//! Device files granted by an OS picker or clipboard gesture, scoped to one guest.
use super::kernel::spawn_device;
use super::{Guest, NativeModuleView, wire};
use gpui_kit::{ClipboardEntry, Context};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{Arc, Mutex},
};

const MAX_CHUNK: usize = 256 << 10;
const MAX_HANDLES: usize = 128;

/// The host's own device work did not complete — a picker, a clipboard, a read
/// off the disk. Every future in this file fails that one way, so the token is
/// stamped here once and each future keeps writing its own sentence.
fn device_failed(error: String) -> wire::Refusal {
    wire::Refusal::new("device_failed", error)
}

/// [`spawn_device`] for this file's futures, which speak sentences.
fn spawn(
    guest: &mut Guest,
    id: u64,
    future: impl std::future::Future<Output = Result<Vec<u8>, String>> + Send + 'static,
) {
    spawn_device(
        guest,
        id,
        async move { future.await.map_err(device_failed) },
    );
}

use super::wire::doors::{self, SelectedFile as FileInfo};

enum Content {
    Disk(Mutex<File>),
    Bytes(Arc<[u8]>),
}
#[derive(Default)]
enum State {
    #[default]
    Retired,
    Active {
        files: HashMap<String, Arc<Content>>,
    },
}

pub(super) struct Filesystem {
    state: Arc<Mutex<State>>,
    pending: Vec<(u64, DeviceRequest)>,
    drops: Option<u64>,
}
enum DeviceRequest {
    Pick,
    ClipboardRead,
    ClipboardWrite(String),
}
impl Default for Filesystem {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Active {
                files: HashMap::new(),
            })),
            pending: Vec::new(),
            drops: None,
        }
    }
}
impl Drop for Filesystem {
    fn drop(&mut self) {
        *self.state.lock().unwrap() = State::Retired;
    }
}

fn grant(
    state: &Mutex<State>,
    name: String,
    content: Content,
    bytes: u64,
) -> Result<FileInfo, String> {
    let mut state = state.lock().unwrap();
    let State::Active { files } = &mut *state else {
        return Err("file grant belongs to a retired guest".into());
    };
    if files.len() >= MAX_HANDLES {
        return Err("too many open file grants".into());
    }
    // Random identities cannot alias a serialized stale grant after another
    // guest or a restarted process picks its first file.
    let token = format!("{:032x}", rand::random::<u128>());
    let collides = files.contains_key(&token);
    if collides {
        return Err("file grant identity collision".into());
    }
    files.insert(token.clone(), Arc::new(content));
    Ok(FileInfo { token, name, bytes })
}
fn grant_path(state: &Mutex<State>, path: PathBuf) -> Result<FileInfo, String> {
    let file = File::open(&path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("the selection is not a file".into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("file name is not UTF-8")?
        .to_owned();
    grant(state, name, Content::Disk(Mutex::new(file)), metadata.len())
}
fn content(state: &Mutex<State>, token: &str) -> Result<Arc<Content>, String> {
    let state = state.lock().unwrap();
    let State::Active { files, .. } = &*state else {
        return Err("file grant belongs to a retired guest".into());
    };
    files
        .get(token)
        .cloned()
        .ok_or_else(|| "unknown file grant".into())
}
fn read_chunk(file: &Content, offset: u64, len: usize) -> Result<Vec<u8>, String> {
    if len == 0 || len > MAX_CHUNK {
        return Err("file read exceeds chunk bounds".into());
    }
    match file {
        Content::Disk(file) => {
            let mut file = file.lock().unwrap();
            file.seek(SeekFrom::Start(offset))
                .map_err(|error| error.to_string())?;
            let mut bytes = vec![0; len];
            let count = file.read(&mut bytes).map_err(|error| error.to_string())?;
            bytes.truncate(count);
            Ok(bytes)
        }
        Content::Bytes(bytes) => {
            let offset = usize::try_from(offset).map_err(|_| "file offset out of range")?;
            let end = offset.saturating_add(len).min(bytes.len());
            Ok(bytes.get(offset..end).unwrap_or_default().to_vec())
        }
    }
}

impl Filesystem {
    pub(super) fn cancel(&mut self, id: u64) {
        self.pending.retain(|(pending, _)| *pending != id);
        if self.drops == Some(id) {
            self.drops = None;
        }
    }
}

pub(super) fn answer(
    guest: &mut Guest,
    capability: &str,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    match (capability, operation) {
        ("fs", "drops") => {
            if let Some(previous) = guest.filesystem.drops.replace(id) {
                guest.reply(previous, Ok(Vec::new()));
            }
        }
        ("fs", "pick") => device(guest, id, DeviceRequest::Pick),
        ("clipboard", "read") => device(guest, id, DeviceRequest::ClipboardRead),
        ("clipboard", "write") => {
            let text = match doors::decode::<String>(payload) {
                Ok(text) => text,
                Err(error) => {
                    guest.refuse(id, "malformed_request", error);
                    return true;
                }
            };
            device(guest, id, DeviceRequest::ClipboardWrite(text));
        }
        ("fs", "read") => {
            let request = doors::decode::<doors::ReadRequest>(payload).and_then(|request| {
                let file = content(&guest.filesystem.state, &request.token)?;
                let len = usize::try_from(request.len).unwrap_or(usize::MAX);
                if len == 0 || len > MAX_CHUNK {
                    return Err("file read exceeds chunk bounds".into());
                }
                Ok((file, request.offset, len))
            });
            match request {
                Ok((file, offset, len)) => spawn(guest, id, async move {
                    tokio::task::spawn_blocking(move || read_chunk(&file, offset, len))
                        .await
                        .map_err(|error| error.to_string())?
                }),
                Err(error) => guest.refuse(id, "malformed_request", error),
            }
        }
        ("fs", "release") => {
            let token = doors::decode::<String>(payload).unwrap_or_default();
            if let State::Active { files, .. } = &mut *guest.filesystem.state.lock().unwrap() {
                files.remove(&token);
            }
            guest.reply(id, Ok(Vec::new()));
        }
        _ => return false,
    }
    true
}

// Called only by the native input route after its guest identity checks. Wire
// observations delivered by a guest cannot mint authority over an OS path.
pub(super) fn observe_drop(guest: &mut Guest, event: &super::wire::Event) -> bool {
    let super::wire::Event::Observation {
        event: super::wire::events::Event::Window(super::wire::events::Window::FileDropped(path)),
        ..
    } = event
    else {
        return false;
    };
    let Some(id) = guest.filesystem.drops else {
        return false;
    };
    let result = grant_path(&guest.filesystem.state, PathBuf::from(path))
        .map(|file| doors::encode(&vec![file]))
        .map_err(device_failed);
    guest.pending.push(super::wire::Event::Response {
        id,
        result,
        done: false,
    });
    true
}

fn device(guest: &mut Guest, id: u64, request: DeviceRequest) {
    if guest.filesystem.pending.len() >= 16 {
        guest.refuse(id, "in_flight_limit", "too many pending device requests");
        return;
    }
    guest.filesystem.pending.push((id, request));
}

pub(super) fn mount(guest: &mut Guest, cx: &mut Context<NativeModuleView>) {
    for (id, request) in std::mem::take(&mut guest.filesystem.pending) {
        match request {
            DeviceRequest::Pick => {
                let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: Some("Choose files".into()),
                });
                let state = guest.filesystem.state.clone();
                spawn(guest, id, async move {
                    let paths = chosen
                        .await
                        .map_err(|error| error.to_string())?
                        .map_err(|error| error.to_string())?
                        .unwrap_or_default();
                    let files = paths
                        .into_iter()
                        .map(|path| grant_path(&state, path))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(doors::encode(&files))
                });
            }
            DeviceRequest::ClipboardRead => {
                let mut text = String::new();
                let mut files = Vec::new();
                let mut failure = None;
                if let Some(item) = cx.read_from_clipboard() {
                    for entry in item.entries() {
                        let granted = match entry {
                            ClipboardEntry::String(value) => {
                                text.push_str(value.text());
                                continue;
                            }
                            ClipboardEntry::ExternalPaths(paths) => paths
                                .paths()
                                .iter()
                                .cloned()
                                .map(|path| grant_path(&guest.filesystem.state, path))
                                .collect::<Result<Vec<_>, _>>(),
                            ClipboardEntry::Image(image) => {
                                let bytes = image.bytes();
                                grant(
                                    &guest.filesystem.state,
                                    format!("pasted.{}", image.format().extension()),
                                    Content::Bytes(bytes.to_vec().into()),
                                    bytes.len() as u64,
                                )
                                .map(|file| vec![file])
                            }
                        };
                        match granted {
                            Ok(granted) => files.extend(granted),
                            Err(error) => {
                                failure = Some(error);
                                break;
                            }
                        }
                    }
                }
                let result = match failure {
                    Some(error) => Err(error),
                    None => Ok(doors::encode(&doors::Clipboard { text, files })),
                };
                guest.reply(id, result.map_err(device_failed));
            }
            DeviceRequest::ClipboardWrite(text) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                guest.reply(id, Ok(Vec::new()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_grants_are_scoped_bounded_and_revoked_on_retirement() {
        let owner = Filesystem::default();
        let another = Filesystem::default();
        let file = grant(
            &owner.state,
            "note".into(),
            Content::Bytes(Arc::from(&b"abcdef"[..])),
            6,
        )
        .unwrap();
        assert!(content(&another.state, &file.token).is_err());
        assert!(content(&owner.state, "/etc/passwd").is_err());
        let granted = content(&owner.state, &file.token).unwrap();
        assert_eq!(read_chunk(&granted, 2, 3).unwrap(), b"cde");
        assert!(read_chunk(&granted, 0, MAX_CHUNK + 1).is_err());
        let retired = owner.state.clone();
        drop(owner);
        assert!(content(&retired, &file.token).is_err());
        assert!(
            grant(
                &retired,
                "late".into(),
                Content::Bytes(Arc::from(&b"x"[..])),
                1
            )
            .is_err()
        );
    }
    #[test]
    fn cancelling_a_drop_subscription_revokes_only_its_own_delivery() {
        let mut filesystem = Filesystem::default();
        filesystem.drops = Some(41);
        filesystem.cancel(40);
        assert_eq!(filesystem.drops, Some(41));
        filesystem.cancel(41);
        assert_eq!(filesystem.drops, None);
    }
    #[test]
    fn reopened_guest_grants_do_not_alias_serialized_tokens() {
        let first = Filesystem::default();
        let old = grant(
            &first.state,
            "a".into(),
            Content::Bytes(Arc::from(&b"a"[..])),
            1,
        )
        .unwrap();
        drop(first);
        let next = Filesystem::default();
        let new = grant(
            &next.state,
            "b".into(),
            Content::Bytes(Arc::from(&b"b"[..])),
            1,
        )
        .unwrap();
        assert_ne!(old.token, new.token);
        assert!(content(&next.state, &old.token).is_err());
    }
    /// A read names a grant token and nothing else; extra bytes after the
    /// request are not a path this door would open.
    #[test]
    fn read_requests_cannot_substitute_paths_for_grants() {
        let mut request = doors::encode(&doors::ReadRequest {
            token: "1".into(),
            offset: 0,
            len: 1,
        });
        request.extend_from_slice(b"/etc/passwd");
        assert!(doors::decode::<doors::ReadRequest>(&request).is_err());
    }
}
