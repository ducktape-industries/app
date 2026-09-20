use serde::{Deserialize, Serialize};

pub const MAX_BLOB_PAGE_BYTES: usize = 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BlobBytesReply {
    pub rev: String,
    pub path: String,
    pub b64: String,
    pub size: i64,
    pub eof: bool,
}
