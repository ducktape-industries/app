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
    pub fn from_accounts<'a>(
        accounts: impl IntoIterator<Item = &'a identity::AccountView>,
    ) -> Self {
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
    pub fn member_label(&self, key_hex: &str) -> String {
        let handle = if key_hex.contains(':') {
            key_hex.to_owned()
        } else {
            format!("user:{key_hex}")
        };
        self.of_handle(&handle)
            .map_or_else(|| short_label(key_hex), str::to_owned)
    }
    fn of_handle(&self, handle: &str) -> Option<&str> {
        match handle.split_once(':') {
            Some(("acct", n)) => self.by_account.get(&n.parse().ok()?).map(String::as_str),
            Some(("user", key)) => self.name_of(key),
            _ => None,
        }
    }
}

static NOBODY_KNOWN: NameDirectory = NameDirectory::empty();

#[derive(Clone, Copy, Debug)]
pub struct ChatReader<'a> {
    pub key: Option<&'a [u8]>,
    pub names: &'a NameDirectory,
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

impl std::hash::Hash for ChatMessage {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let Self {
            id,
            view_key: _,
            seq,
            author,
            meta,
            body: _,
            edit_body: _,
            blocks: _,
            pending,
            rev,
            edited,
            deleted,
            reply_count,
            thread_seq,
            show_author,
            initial,
            avatar_kind,
            height,
            time,
            reactions,
            render_rev,
        } = self;
        id.hash(state);
        seq.hash(state);
        author.hash(state);
        meta.hash(state);
        pending.hash(state);
        rev.hash(state);
        edited.hash(state);
        deleted.hash(state);
        reply_count.hash(state);
        thread_seq.hash(state);
        show_author.hash(state);
        initial.hash(state);
        avatar_kind.hash(state);
        height.hash(state);
        time.hash(state);
        reactions.hash(state);
        render_rev.hash(state);
    }
}

impl ChatMessage {
    fn seed_render_rev(mut self) -> Self {
        use std::hash::{Hash as _, Hasher as _};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        self.blocks.hash(&mut hasher);
        self.render_rev = i64::from_ne_bytes(hasher.finish().to_ne_bytes());
        self
    }

    fn bump_render_rev(&mut self) {
        self.render_rev = self.render_rev.wrapping_add(1);
    }
}

fn next_message_view_key() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};

    static NEXT: AtomicI64 = AtomicI64::new(1);
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |key| {
        key.checked_add(1)
    })
    .expect("a process cannot render more than i64::MAX rows")
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
        view_key: next_message_view_key(),
        seq: number_i64(row.seq),
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
        rev: i64::from(row.rev),
        edited: row.rev > 0,
        deleted,
        reply_count: number_i64(row.reply_count),
        thread_seq: number_i64(row.thread.unwrap_or_default()),
        show_author: true,
        initial: avatar_initial(&row.author, reader.names),
        avatar_kind: avatar_kind(&row.author, reader.names).into(),
        height: number_i64(row.height),
        time: number_i64(row.time),
        reactions: row
            .reactions
            .into_iter()
            .map(|reaction| ChatReaction {
                emoji: reaction.emoji,
                count: count_i64(reaction.reactors.len()),
                reacted_by_me: reaction
                    .reactors
                    .iter()
                    .any(|reactor| reader.is_this_key(reactor)),
                reactors: reaction.reactors,
            })
            .collect(),
        render_rev: 0,
    }
    .seed_render_rev()
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
        if message.show_author != show {
            message.show_author = show;
            message.bump_render_rev();
        }
    }
}

pub fn blocks_view(blocks: &[Block], chain: &ChainId) -> Vec<ChatBlock> {
    blocks_view_with_names(blocks, &NOBODY_KNOWN, chain)
}
pub fn blocks_view_with_names(
    blocks: &[Block],
    names: &NameDirectory,
    chain: &ChainId,
) -> Vec<ChatBlock> {
    named_blocks(blocks, names)
        .iter()
        .map(|block| block_view(block, chain))
        .collect()
}
pub fn paragraph_blocks(text: &str, chain: &ChainId) -> Vec<ChatBlock> {
    blocks_view(&super::parse_message(text), chain)
}
pub fn message_body_with_names(blocks: &[Block], names: &NameDirectory) -> String {
    message_body(&named_blocks(blocks, names))
}

fn named_blocks(blocks: &[Block], names: &NameDirectory) -> Vec<Block> {
    let mut blocks = blocks.to_vec();
    for block in &mut blocks {
        let spans = match block {
            Block::Paragraph(spans) | Block::Quote(spans) => spans,
            Block::Code { .. } | Block::Divider => continue,
        };
        for span in spans {
            if let Some(party) = span.marks.iter().find_map(|mark| match mark {
                Mark::Mention(party) => Some(party),
                _ => None,
            }) {
                span.text = mention_label(party, names);
            }
        }
    }
    blocks
}

fn message_body(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(spans) => span_text(spans),
            Block::Code { lang, text } => match lang {
                Some(lang) => format!("{lang}\n{text}"),
                None => text.clone(),
            },
            Block::Quote(spans) => format!("“{}”", span_text(spans)),
            Block::Divider => "────────".into(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn span_text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

fn block_view(block: &Block, chain: &ChainId) -> ChatBlock {
    match block {
        Block::Paragraph(spans) => rich_block("paragraph", spans, chain),
        Block::Quote(spans) => rich_block("quote", spans, chain),
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
fn rich_block(kind: &str, spans: &[Span], chain: &ChainId) -> ChatBlock {
    let rich = spans.iter().any(|span| !span.marks.is_empty());
    ChatBlock {
        kind: kind.into(),
        text: span_text(spans),
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
    if span.text.is_empty() {
        return out;
    }
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

fn draft_body(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(spans) => draft_spans(spans),
            Block::Quote(spans) => format!("> {}", draft_spans(spans)),
            Block::Code { lang, text } => {
                format!("```{}\n{text}\n```", lang.as_deref().unwrap_or_default())
            }
            Block::Divider => "---".into(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn draft_spans(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|span| {
            let mut text = span
                .marks
                .iter()
                .find_map(|mark| match mark {
                    Mark::Mention(party) => Some(mention_token(party)),
                    _ => None,
                })
                .unwrap_or_else(|| span.text.clone());
            for mark in &span.marks {
                text = match mark {
                    Mark::Bold => format!("**{text}**"),
                    Mark::Italic => format!("_{text}_"),
                    Mark::Link(url) => format!("[{text}]({url})"),
                    Mark::Mention(_) => text,
                };
            }
            text
        })
        .collect()
}

fn mention_token(party: &Party) -> String {
    match party {
        Party::Account(account) => format!("<@{account}>"),
        Party::Key(key) => format!("<@key:{}>", hex_encode(key)),
        Party::Module(_) | Party::System => String::new(),
    }
}

fn mention_label(party: &Party, names: &NameDirectory) -> String {
    match party {
        Party::Account(account) => names
            .by_account
            .get(account)
            .filter(|name| !name.is_empty())
            .map_or_else(|| format!("@account-{account}"), |name| format!("@{name}")),
        Party::Key(key) => format!("@{}", names.member_label(&hex_encode(key))),
        Party::Module(module) => format!("@{module}"),
        Party::System => "@system".into(),
    }
}
fn avatar_source(author: &str, names: &NameDirectory) -> String {
    match author.split_once(':') {
        Some(("user", key)) => names.member_label(key),
        Some(("acct", _)) => author_display(author, names),
        Some(("module", module)) => module.to_owned(),
        _ => "system".into(),
    }
}

fn avatar_initial(author: &str, names: &NameDirectory) -> String {
    initial_of(&avatar_source(author, names))
}

fn avatar_kind(author: &str, names: &NameDirectory) -> &'static str {
    match author.split_once(':') {
        Some(("user", _)) => "human",
        Some(("acct", number)) => match number.parse::<u64>() {
            Ok(number) if names.programs.contains(&number) => "agent",
            _ => "human",
        },
        Some(_) | None => "agent",
    }
}

fn initial_of(source: &str) -> String {
    source
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

fn number_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn count_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(author: &str) -> index::MsgRow {
        index::MsgRow {
            channel_id: "general".into(),
            seq: 7,
            message_id: "message-7".into(),
            author: author.into(),
            height: 11,
            time: 12,
            blocks: vec![Block::paragraph("hello")],
            text: "hello".into(),
            deleted: false,
            edited: false,
            rev: 0,
            edited_at: None,
            base_rev: None,
            thread: None,
            reply_count: 0,
            last_reply_seq: None,
            reactions: Vec::new(),
            tags: Vec::new(),
        }
    }

    fn program_account() -> identity::AccountView {
        identity::AccountView {
            number: 7,
            name: "quackbot".into(),
            control: identity::Control::Program {
                controller: 1,
                executor: "agent".into(),
                generation: 0,
                standing: identity::ProgramStanding::Active,
            },
            keys: Vec::new(),
            avatar: None,
            bio: None,
            updated_at: 0,
        }
    }

    #[test]
    fn committed_rows_get_fresh_view_identity_and_render_seed() {
        let chain: ChainId = "dognet#b5b6ea90".parse().unwrap();
        let names = NameDirectory::empty();
        let reader = ChatReader::new(None, &names);
        let first = chat_message(row("system"), reader, &chain);
        let second = chat_message(row("system"), reader, &chain);
        assert_ne!(first.view_key, second.view_key);
        assert_ne!(first.render_rev, 0);
        assert_eq!(first.render_rev, second.render_rev);
    }

    #[test]
    fn program_and_system_authors_use_agent_avatars() {
        let program = program_account();
        let names = NameDirectory::from_accounts(&[program]);
        let chain: ChainId = "dognet#b5b6ea90".parse().unwrap();
        let program_message = chat_message(row("acct:7"), ChatReader::new(None, &names), &chain);
        let system_message = chat_message(row("system"), ChatReader::new(None, &names), &chain);
        assert_eq!(program_message.avatar_kind, "agent");
        assert_eq!(program_message.initial, "Q");
        assert_eq!(system_message.avatar_kind, "agent");
        assert_eq!(system_message.initial, "S");
    }

    #[test]
    fn named_mentions_render_for_display_and_keep_edit_tokens() {
        let program = program_account();
        let names = NameDirectory::from_accounts(&[program]);
        let chain: ChainId = "dognet#b5b6ea90".parse().unwrap();
        let mut message = row("system");
        message.blocks = vec![Block::Paragraph(vec![Span {
            text: "<@7>".into(),
            marks: vec![Mark::Mention(Party::Account(7))],
        }])];
        let rendered = chat_message(message, ChatReader::new(None, &names), &chain);
        assert_eq!(rendered.body, "@quackbot");
        assert_eq!(rendered.blocks[0].spans[1].mention, "@quackbot");
        assert_eq!(rendered.edit_body, "<@7>");
    }
}
