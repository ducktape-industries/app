use super::{Block, Mark, Party, Span, index};
use crate::interfaces::identity;
use duck_address::ChainId;
use duck_address::identity::AccountAddress;
use std::collections::{BTreeMap, BTreeSet};

pub const CHAT_HOT_WINDOW_LIMIT: usize = 256;

#[derive(Clone, Debug, Hash, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ChatChannel {
    pub id: String,
    pub name: String,
    pub archived: bool,
    pub members_only: bool,
    pub huddle_count: i64,
    pub head_seq: i64,
    pub huddle: Vec<HuddleSeat>,
    pub voice: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct HuddleSeat {
    pub label: String,
    pub initials: String,
    pub is_you: bool,
    pub node: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Default, serde::Serialize)]
pub struct ChatReaction {
    pub emoji: String,
    pub count: i64,
    pub reacted_by_me: bool,
    pub reactors: Vec<String>,
}

#[derive(Clone, Debug, Hash, PartialEq, Default, serde::Serialize)]
pub struct ChatBlock {
    pub kind: String,
    pub text: String,
    pub lang: String,
    pub rich: bool,
    pub spans: Vec<ChatSpan>,
}

#[derive(Clone, Debug, Hash, PartialEq, Default, serde::Serialize)]
pub struct ChatSpan {
    pub mention: String,
    pub mention_link: String,
    pub link_text: String,
    pub link: String,
    pub bold_italic: String,
    pub bold: String,
    pub italic: String,
    pub plain: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct BoundAccount {
    pub number: u64,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct NameDirectory {
    accounts: BTreeMap<String, BoundAccount>,
    by_account: BTreeMap<u64, String>,
    programs: BTreeSet<u64>,
}

impl NameDirectory {
    pub const fn empty() -> Self {
        Self {
            accounts: BTreeMap::new(),
            by_account: BTreeMap::new(),
            programs: BTreeSet::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.by_account.is_empty()
    }
    pub fn from_accounts(accounts: &[identity::AccountView]) -> Self {
        let mut names = Self::empty();
        for account in accounts {
            names
                .by_account
                .insert(account.number, account.name.clone());
            if matches!(account.control, identity::Control::Program { .. }) {
                names.programs.insert(account.number);
            }
            for key in &account.keys {
                names.accounts.insert(
                    hex_encode(&key.pubkey),
                    BoundAccount {
                        number: account.number,
                        name: account.name.clone(),
                    },
                );
            }
        }
        names
    }
    pub fn name_of(&self, key_hex: &str) -> Option<&str> {
        self.accounts.get(key_hex).map(|a| a.name.as_str())
    }
    pub fn account_of(&self, key_hex: &str) -> Option<u64> {
        self.accounts.get(key_hex).map(|a| a.number)
    }
    pub fn member_label(&self, key_hex: &str) -> String {
        let handle = if key_hex.contains(':') {
            key_hex.to_owned()
        } else {
            format!("user:{key_hex}")
        };
        self.of_handle(&handle)
            .map_or_else(|| short_label(key_hex), str::to_owned)
    }
    pub fn handle_of(&self, key: &[u8]) -> String {
        index::party_handle(&self.party_of(key))
    }
    pub fn party_of(&self, key: &[u8]) -> Party {
        self.account_of(&hex_encode(key))
            .map_or_else(|| Party::Key(key.to_vec()), Party::Account)
    }
    pub fn owns_handle(&self, handle: &str, key: &[u8]) -> bool {
        handle == self.handle_of(key) || handle == index::party_handle(&Party::Key(key.to_vec()))
    }
    fn of_handle(&self, handle: &str) -> Option<&str> {
        match handle.split_once(':') {
            Some(("acct", n)) => self.by_account.get(&n.parse().ok()?).map(String::as_str),
            Some(("user", key)) => self.name_of(key),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ChatReader<'a> {
    pub key: Option<&'a [u8]>,
    pub names: &'a NameDirectory,
}
impl ChatReader<'static> {
    pub fn nobody() -> Self {
        static EMPTY: NameDirectory = NameDirectory::empty();
        Self {
            key: None,
            names: &EMPTY,
        }
    }
}
impl<'a> ChatReader<'a> {
    pub fn new(key: Option<&'a [u8]>, names: &'a NameDirectory) -> Self {
        Self { key, names }
    }
    fn is_this_key(&self, handle: &str) -> bool {
        self.key
            .is_some_and(|key| handle == index::party_handle(&Party::Key(key.to_vec())))
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ChatMessage {
    pub id: String,
    pub view_key: i64,
    pub seq: i64,
    pub author: String,
    pub meta: String,
    pub body: String,
    pub edit_body: String,
    pub blocks: Vec<ChatBlock>,
    pub pending: bool,
    pub rev: i64,
    pub edited: bool,
    pub deleted: bool,
    pub reply_count: i64,
    pub thread_seq: i64,
    pub show_author: bool,
    pub initial: String,
    pub avatar_kind: String,
    pub height: i64,
    pub time: i64,
    pub reactions: Vec<ChatReaction>,
    pub render_rev: i64,
}

pub fn replace_channel(
    mut channels: Vec<ChatChannel>,
    id: &str,
    channel: ChatChannel,
) -> Vec<ChatChannel> {
    if let Some(current) = channels.iter_mut().find(|current| current.id == id) {
        *current = channel;
    } else {
        channels.push(channel);
    }
    channels
}
pub fn advance_channel_head(
    mut channels: Vec<ChatChannel>,
    id: &str,
    seq: i64,
) -> Vec<ChatChannel> {
    if let Some(channel) = channels.iter_mut().find(|channel| channel.id == id) {
        channel.head_seq = channel.head_seq.max(seq);
    }
    channels
}

pub fn chat_message(row: index::MsgRow, reader: ChatReader<'_>, chain: &ChainId) -> ChatMessage {
    let deleted = row.deleted;
    let blocks = if deleted {
        vec![ChatBlock {
            kind: "paragraph".into(),
            text: "Message deleted".into(),
            ..ChatBlock::default()
        }]
    } else {
        blocks_view_with_names(&row.blocks, reader.names, chain)
    };
    ChatMessage {
        id: row.message_id,
        view_key: row.seq as i64,
        seq: row.seq as i64,
        author: author_display(&row.author, reader.names),
        meta: if row.rev > 0 {
            format!("#{} · edited", row.seq)
        } else {
            format!("#{}", row.seq)
        },
        body: if deleted {
            "Message deleted".into()
        } else {
            message_body_with_names(&row.blocks, reader.names)
        },
        edit_body: if deleted {
            String::new()
        } else {
            draft_body(&row.blocks)
        },
        blocks,
        pending: false,
        rev: row.rev as i64,
        edited: row.rev > 0,
        deleted,
        reply_count: row.reply_count as i64,
        thread_seq: row.thread.unwrap_or_default() as i64,
        show_author: true,
        initial: initial(&row.author, reader.names),
        avatar_kind: "human".into(),
        height: row.height as i64,
        time: row.time as i64,
        reactions: row
            .reactions
            .into_iter()
            .map(|reaction| ChatReaction {
                emoji: reaction.emoji,
                count: reaction.reactors.len() as i64,
                reacted_by_me: reaction
                    .reactors
                    .iter()
                    .any(|reactor| reader.is_this_key(reactor)),
                reactors: reaction.reactors,
            })
            .collect(),
        render_rev: 0,
    }
}

pub fn mark_message_groups(messages: &mut [ChatMessage]) {
    let shows: Vec<_> = messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            i == 0 || m.deleted || messages[i - 1].deleted || messages[i - 1].author != m.author
        })
        .collect();
    for (message, show) in messages.iter_mut().zip(shows) {
        message.show_author = show;
    }
}

pub fn blocks_view(blocks: &[Block], chain: &ChainId) -> Vec<ChatBlock> {
    blocks_view_with_names(blocks, &NameDirectory::empty(), chain)
}
pub fn blocks_view_with_names(
    blocks: &[Block],
    names: &NameDirectory,
    chain: &ChainId,
) -> Vec<ChatBlock> {
    blocks
        .iter()
        .map(|block| block_view(block, names, chain))
        .collect()
}
pub fn paragraph_blocks(text: &str, chain: &ChainId) -> Vec<ChatBlock> {
    blocks_view(&super::parse_message(text), chain)
}
pub fn message_body_with_names(blocks: &[Block], names: &NameDirectory) -> String {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(spans) | Block::Quote(spans) => spans
                .iter()
                .map(|span| mention_text(span, names))
                .collect::<String>(),
            Block::Code { text, .. } => text.clone(),
            Block::Divider => "────────".into(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn block_view(block: &Block, names: &NameDirectory, chain: &ChainId) -> ChatBlock {
    match block {
        Block::Paragraph(spans) => rich_block("paragraph", spans, names, chain),
        Block::Quote(spans) => rich_block("quote", spans, names, chain),
        Block::Code { lang, text } => ChatBlock {
            kind: "code".into(),
            text: text.clone(),
            lang: lang.clone().unwrap_or_default(),
            ..ChatBlock::default()
        },
        Block::Divider => ChatBlock {
            kind: "divider".into(),
            ..ChatBlock::default()
        },
    }
}
fn rich_block(kind: &str, spans: &[Span], names: &NameDirectory, chain: &ChainId) -> ChatBlock {
    let rich = spans.iter().any(|span| !span.marks.is_empty());
    ChatBlock {
        kind: kind.into(),
        text: spans.iter().map(|span| mention_text(span, names)).collect(),
        rich,
        spans: if rich {
            spans
                .iter()
                .flat_map(|span| span_views(span, chain))
                .collect()
        } else {
            Vec::new()
        },
        ..ChatBlock::default()
    }
}
fn span_views(span: &Span, chain: &ChainId) -> Vec<ChatSpan> {
    let mut out = Vec::new();
    if let Some(mark) = span
        .marks
        .iter()
        .find(|mark| matches!(mark, Mark::Mention(_)))
    {
        out.push(ChatSpan {
            plain: "\u{2009}".into(),
            ..ChatSpan::default()
        });
        let mut mention = ChatSpan {
            mention: span.text.clone(),
            ..ChatSpan::default()
        };
        if let Mark::Mention(Party::Account(account)) = mark {
            mention.mention_link = duck_account_link(chain, *account);
        }
        out.push(mention);
        out.push(ChatSpan {
            plain: "\u{2009}".into(),
            ..ChatSpan::default()
        });
        return out;
    }
    let mut value = ChatSpan::default();
    if let Some(Mark::Link(url)) = span.marks.iter().find(|mark| matches!(mark, Mark::Link(_))) {
        value.link_text = span.text.clone();
        value.link = url.clone();
    } else {
        let bold = span.marks.iter().any(|mark| matches!(mark, Mark::Bold));
        let italic = span.marks.iter().any(|mark| matches!(mark, Mark::Italic));
        match (bold, italic) {
            (true, true) => value.bold_italic = span.text.clone(),
            (true, false) => value.bold = span.text.clone(),
            (false, true) => value.italic = span.text.clone(),
            (false, false) => value.plain = span.text.clone(),
        };
    }
    out.push(value);
    out
}
fn mention_text(span: &Span, names: &NameDirectory) -> String {
    if let Some(Mark::Mention(party)) = span
        .marks
        .iter()
        .find(|mark| matches!(mark, Mark::Mention(_)))
    {
        match party {
            Party::Account(account) => names
                .of_handle(&format!("acct:{account}"))
                .unwrap_or("account")
                .to_owned(),
            Party::Key(key) => names.member_label(&hex_encode(key)),
            Party::Module(module) => module.clone(),
            Party::System => "system".into(),
        }
    } else {
        span.text.clone()
    }
}
fn draft_body(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(spans) => spans.iter().map(|span| span.text.clone()).collect(),
            Block::Quote(spans) => format!(
                "> {}",
                spans
                    .iter()
                    .map(|span| span.text.clone())
                    .collect::<String>()
            ),
            Block::Code { lang, text } => {
                format!("```{}\n{text}\n```", lang.as_deref().unwrap_or_default())
            }
            Block::Divider => "---".into(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn initial(author: &str, names: &NameDirectory) -> String {
    author_display(author, names)
        .chars()
        .find(char::is_ascii_alphanumeric)
        .map_or_else(|| "•".into(), |c| c.to_ascii_uppercase().to_string())
}

pub fn author_display(author: &str, names: &NameDirectory) -> String {
    names
        .of_handle(author)
        .map_or_else(|| author_name(author), str::to_owned)
}
pub fn author_name(author: &str) -> String {
    match author.split_once(':') {
        Some(("user", key)) => format!("user {}", short_label(key)),
        Some(("acct", account)) => format!("account {account}"),
        Some(("module", module)) => module.into(),
        _ => "system".into(),
    }
}
pub fn short_label(id: &str) -> String {
    let mut out: String = id.chars().take(8).collect();
    if id.chars().count() > 8 {
        out.push('…');
    }
    out
}
pub fn duck_account_link(chain: &ChainId, account: u64) -> String {
    AccountAddress { account }
        .address(chain.clone())
        .map(|address| address.to_string())
        .unwrap_or_default()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
