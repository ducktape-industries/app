use super::{Block, HuddleEntry, Party, PostPolicy};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MsgRow {
    pub channel_id: String,
    pub seq: u64,
    pub message_id: String,
    pub author: String,
    pub height: u64,
    pub time: u64,
    pub blocks: Vec<Block>,
    pub text: String,
    pub deleted: bool,
    pub edited: bool,
    pub rev: u32,
    pub edited_at: Option<u64>,
    pub base_rev: Option<u32>,
    pub thread: Option<u64>,
    pub reply_count: u64,
    pub last_reply_seq: Option<u64>,
    pub reactions: Vec<ReactionRow>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReactionRow {
    pub emoji: String,
    pub reactors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelRow {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub post_policy: PostPolicy,
    pub owner: String,
    pub archived: bool,
    pub hooks: Vec<String>,
    pub huddle: Vec<HuddleEntry>,
    pub voice: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberRow {
    pub party: String,
    pub height: u64,
    pub time: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelInfo {
    #[serde(flatten)]
    pub channel: ChannelRow,
    pub head_seq: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagRow {
    pub tag: String,
    pub count: u64,
    pub last_seq: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatViewQuery {
    Channels {
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    Channel {
        channel_id: String,
    },
    Roots {
        channel_id: String,
        #[serde(default)]
        before_seq: Option<u64>,
        #[serde(default)]
        limit: Option<usize>,
    },
    MessagesAround {
        channel_id: String,
        seq: u64,
        #[serde(default)]
        limit: Option<usize>,
    },
    Message {
        message_id: String,
    },
    Revisions {
        channel_id: String,
        seq: u64,
    },
    Thread {
        channel_id: String,
        root_seq: u64,
        #[serde(default)]
        after_reply_seq: Option<u64>,
        #[serde(default)]
        limit: Option<usize>,
    },
    Reactions {
        channel_id: String,
        seq: u64,
    },
    Members {
        channel_id: String,
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    Search {
        text: String,
        #[serde(default)]
        channel_id: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    Tags {
        #[serde(default)]
        channel_id: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    TagSearch {
        tag: String,
        #[serde(default)]
        channel_id: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatViewReply {
    Channels {
        channels: Vec<ChannelInfo>,
        has_more: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_after: Option<String>,
    },
    Channel(Option<ChannelInfo>),
    Roots {
        roots: Vec<MsgRow>,
        has_more: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_before_seq: Option<u64>,
    },
    Messages(Vec<MsgRow>),
    Message(Option<MsgRow>),
    Revisions(Vec<MsgRow>),
    Thread {
        root: Option<MsgRow>,
        replies: Vec<MsgRow>,
        has_more: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_reply_seq: Option<u64>,
    },
    Reactions(Vec<ReactionRow>),
    Members {
        members: Vec<MemberRow>,
        has_more: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        next_after: Option<String>,
    },
    Hits(Vec<MsgRow>),
    Tags(Vec<TagRow>),
}

pub fn party_handle(party: &Party) -> String {
    match party {
        Party::Account(account) => format!("acct:{account}"),
        Party::Key(key) => format!("user:{}", user_handle(key)),
        Party::Module(module) => format!("module:{module}"),
        Party::System => "system".to_string(),
    }
}

fn user_handle(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes)
        && !text.is_empty()
        && !text.chars().any(char::is_control)
    {
        return text.to_owned();
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
