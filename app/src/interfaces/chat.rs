use serde::{Deserialize, Serialize};

pub const HUDDLE_JOIN_NS: &[u8] = b"ducktape/huddle-join/v1";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PostPolicy {
    Open,
    MembersOnly,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Party {
    Account(u64),
    Key(Vec<u8>),
    Module(String),
    System,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Mark {
    Bold,
    Italic,
    Link(String),
    Mention(Party),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub text: String,
    pub marks: Vec<Mark>,
}
impl Span {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            marks: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HuddleEntry {
    pub party: String,
    pub node: String,
    pub joined_at: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    Paragraph(Vec<Span>),
    Code { lang: Option<String>, text: String },
    Quote(Vec<Span>),
    Divider,
}
impl Block {
    pub fn paragraph(text: impl Into<String>) -> Self {
        Self::Paragraph(vec![Span::plain(text)])
    }
}

pub fn parse_message(input: &str) -> Vec<Block> {
    let lines: Vec<_> = input.lines().collect();
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            let lang = trimmed.trim_start_matches('`').trim();
            let mut end = index + 1;
            while end < lines.len() && lines[end].trim() != "```" {
                end += 1;
            }
            blocks.push(Block::Code {
                lang: (!lang.is_empty()).then_some(lang.to_owned()),
                text: lines[index + 1..end].join("\n"),
            });
            index = if end < lines.len() { end + 1 } else { end };
        } else if matches!(trimmed, "---" | "***") {
            blocks.push(Block::Divider);
            index += 1;
        } else if let Some(quote) = trimmed.strip_prefix('>') {
            blocks.push(Block::Quote(inline_spans(quote.trim_start())));
            index += 1;
        } else if trimmed.is_empty() {
            index += 1;
        } else {
            blocks.push(Block::Paragraph(inline_spans(trimmed)));
            index += 1;
        }
    }
    if blocks.is_empty() {
        blocks.push(Block::paragraph(input.trim().to_owned()));
    }
    blocks
}

pub fn inline_spans(text: &str) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut index = 0;
    while index < chars.len() {
        if let Some((party, consumed)) = mention_at(&chars, index) {
            flush_plain(&mut plain, &mut spans);
            spans.push(Span {
                text: chars[index..index + consumed].iter().collect(),
                marks: vec![Mark::Mention(party)],
            });
            index += consumed;
        } else if let Some((inner, consumed)) =
            fenced(&chars, index, "**").or_else(|| fenced(&chars, index, "__"))
        {
            flush_plain(&mut plain, &mut spans);
            spans.extend(inline_spans(&inner).into_iter().map(|mut span| {
                span.marks.push(Mark::Bold);
                span
            }));
            index += consumed;
        } else if let Some((inner, consumed)) =
            fenced(&chars, index, "*").or_else(|| fenced(&chars, index, "_"))
        {
            flush_plain(&mut plain, &mut spans);
            spans.extend(inline_spans(&inner).into_iter().map(|mut span| {
                span.marks.push(Mark::Italic);
                span
            }));
            index += consumed;
        } else if let Some(length) = url_len(&chars, index) {
            flush_plain(&mut plain, &mut spans);
            let target: String = chars[index..index + length].iter().collect();
            spans.push(Span {
                text: target.clone(),
                marks: vec![Mark::Link(target)],
            });
            index += length;
        } else {
            plain.push(chars[index]);
            index += 1;
        }
    }
    flush_plain(&mut plain, &mut spans);
    if spans.is_empty() {
        spans.push(Span::plain(String::new()));
    }
    spans
}

fn flush_plain(plain: &mut String, spans: &mut Vec<Span>) {
    if !plain.is_empty() {
        spans.push(Span::plain(std::mem::take(plain)));
    }
}

fn mention_at(chars: &[char], at: usize) -> Option<(Party, usize)> {
    if chars.get(at) != Some(&'<') || chars.get(at + 1) != Some(&'@') {
        return None;
    }
    let end = chars[at + 2..].iter().position(|c| *c == '>')? + at + 2;
    let id: String = chars[at + 2..end].iter().collect();
    let party = if let Some(key) = id.strip_prefix("key:") {
        Party::Key(decode_hex(key)?)
    } else if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        Party::Account(id.parse().ok()?)
    } else {
        return None;
    };
    Some((party, end + 1 - at))
}

fn fenced(chars: &[char], at: usize, marker: &str) -> Option<(String, usize)> {
    let marker: Vec<_> = marker.chars().collect();
    if !chars[at..].starts_with(&marker) {
        return None;
    }
    let start = at + marker.len();
    if chars
        .get(start)
        .is_none_or(|character| character.is_whitespace())
    {
        return None;
    }
    for cursor in start..=chars.len().saturating_sub(marker.len()) {
        if chars[cursor..].starts_with(&marker)
            && cursor > start
            && !chars[cursor - 1].is_whitespace()
        {
            return Some((
                chars[start..cursor].iter().collect(),
                cursor + marker.len() - at,
            ));
        }
    }
    None
}

fn url_len(chars: &[char], at: usize) -> Option<usize> {
    let rest: String = chars[at..].iter().collect();
    if !["http://", "https://", "duck://"]
        .iter()
        .any(|prefix| rest.starts_with(prefix))
    {
        return None;
    }
    let mut len = chars[at..]
        .iter()
        .take_while(|c| !c.is_whitespace())
        .count();
    while len > 0 && chars[at..at + len].last() == Some(&')') {
        len -= 1;
    }
    (len > 0).then_some(len)
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.is_empty()
        || !value.len().is_multiple_of(2)
        || !value.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&value[at..at + 2], 16).ok())
        .collect()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatMsg {
    CreateChannel {
        channel_id: String,
        name: String,
        post_policy: PostPolicy,
    },
    CreateVoiceChannel {
        channel_id: String,
        name: String,
    },
    CreateDmChannel {
        counterpart: u64,
        name: String,
    },
    RenameChannel {
        channel_id: String,
        name: String,
    },
    SetChannelArchived {
        channel_id: String,
        archived: bool,
    },
    PostMessage {
        channel_id: String,
        message_id: String,
        blocks: Vec<Block>,
        thread: Option<u64>,
    },
    EditMessage {
        channel_id: String,
        seq: u64,
        blocks: Vec<Block>,
        base_rev: Option<u32>,
    },
    DeleteMessage {
        channel_id: String,
        seq: u64,
    },
    AddReaction {
        channel_id: String,
        seq: u64,
        emoji: String,
    },
    RemoveReaction {
        channel_id: String,
        seq: u64,
        emoji: String,
    },
    RegisterHook {
        channel_id: String,
        module_id: String,
    },
    UnregisterHook {
        channel_id: String,
        module_id: String,
    },
    SetMembership {
        channel_id: String,
        party: Party,
        member: bool,
    },
    JoinHuddle {
        channel_id: String,
        node: Vec<u8>,
        node_proof: Vec<u8>,
    },
    LeaveHuddle {
        channel_id: String,
    },
    SweepHuddle {
        channel_id: String,
        party: Party,
    },
}

pub fn encode_msg(message: &ChatMsg) -> Vec<u8> {
    sdk::wire::encode(message)
}
pub fn decode_msg(bytes: &[u8]) -> Result<ChatMsg, String> {
    sdk::wire::decode(bytes)
}
pub fn huddle_join_preimage(channel_id: &str, user: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    sdk::codec::push_str(&mut out, channel_id);
    sdk::codec::push_bytes(&mut out, user);
    out
}

pub mod client;
pub mod index;

#[cfg(test)]
mod owner_golden;
