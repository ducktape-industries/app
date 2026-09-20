//! The `duck://` address, read. The grammar is sdk's `duck-address` and
//! nothing here spells a second one:
//!
//! ```text
//! duck://<label>-<salt>/<module>/<module-path…>
//! ```
//!
//! [`classify_duck_link`] maps a parsed address onto the open plane's kinds
//! through each module's typed tail (`PageAddress`, `MessageAddress`,
//! `FileAddress`, `RunAddress`, `ForgeRepoAddress` / `ForgeLocator`,
//! `AccountAddress`): every
//! surface that opens or embeds a link classifies through it and nowhere
//! else. What names nothing is [`DuckKind::Unknown`] carrying the sentence
//! why — never an error here; the caller decides what "nothing to open"
//! looks like.
//!
//! THE AUTHORITY IS THE NETWORK. [`resolve_duck_link`] adds the scope check
//! `classify_duck_link` cannot make on its own: the address's chain id
//! against the connected one, compared as the crate's `ChainId`.
//!
//! THE OLD FORM IS NOT READ (core #2637, ruling Q4). `duck://<module>/…
//! [?net=<digest>]` and the chain-less `duck://account/<n>` are refused with
//! [`OLD_FORM`], never guessed at: a stored link of that form opens nothing
//! rather than the wrong thing.

pub(crate) use crate::DuckKind;
use crate::interfaces::files::FileAddress;
use duck_address::chat::MessageAddress;
use duck_address::forge::{ForgeLocator, ForgeRepoAddress, ForgeTarget};
use duck_address::identity::AccountAddress;
use duck_address::pages::PageAddress;
use duck_address::runs::RunAddress;
use duck_address::{Address, ChainId, Refused};

/// What a `duck://` link that is not the chain-id grammar is refused with.
pub const OLD_FORM: &str = "this link uses an address form this app no longer reads";

/// One classified link. Only the fields its `kind` names are meaningful;
/// the rest are empty / zero.
#[derive(Clone, Debug, PartialEq)]
pub struct DuckLink {
    pub kind: DuckKind,
    /// `forge_*`: the repository name, `<owner>/<repo>`.
    pub repo: String,
    /// `forge_item`: the item number.
    pub number: i64,
    /// `channel_message`: the message seq; `forge_item`: the comment seq, or 0.
    pub seq: i64,
    /// `page`: the page id.
    pub page: String,
    /// `page`: the block the link lands on, or "" for the page's top.
    pub block: String,
    /// `run`: the run's dispatch digest.
    pub dispatch: String,
    /// `channel` / `channel_message`: the channel id.
    pub channel: String,
    /// `files`: the absolute duckfs path; `forge_blob`: the repo-relative path.
    pub path: String,
    /// `forge_blob`: the commit the file is read at.
    pub rev: String,
    /// `account`: the account number, as the DM peer list keys it.
    pub account: String,
    /// The network the address names; `None` for a link that names none (a
    /// web link, `Unknown`). `foreign_network` carries the one that did NOT
    /// match.
    pub chain: Option<ChainId>,
    /// `unknown`: why it opens nothing, as a sentence to show.
    pub refusal: String,
}

impl DuckLink {
    fn unknown(refusal: impl Into<String>) -> Self {
        Self {
            kind: DuckKind::Unknown,
            repo: String::new(),
            number: 0,
            seq: 0,
            page: String::new(),
            block: String::new(),
            dispatch: String::new(),
            channel: String::new(),
            path: String::new(),
            rev: String::new(),
            account: String::new(),
            chain: None,
            refusal: refusal.into(),
        }
    }

    pub(crate) fn of(kind: DuckKind) -> Self {
        Self {
            kind,
            ..Self::unknown("")
        }
    }
}

impl From<Refused> for DuckLink {
    fn from(refused: Refused) -> Self {
        DuckLink::unknown(refused.sentence)
    }
}

/// Classify one link. Web links (`http(s)://`) are [`DuckKind::Web`];
/// everything that is not a well-formed address is [`DuckKind::Unknown`].
pub fn classify_duck_link(url: String) -> DuckLink {
    let web = url.starts_with("http://") || url.starts_with("https://");
    if web {
        return DuckLink::of(DuckKind::Web);
    }
    let Some(rest) = url.strip_prefix("duck://") else {
        return DuckLink::unknown("this link names nothing the app can open");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.parse::<ChainId>().is_err() {
        return DuckLink::unknown(OLD_FORM);
    }
    let address = match Address::parse(&url) {
        Ok(address) => address,
        Err(refused) => return refused.into(),
    };
    match typed(&address) {
        Ok(link) => DuckLink {
            chain: Some(address.chain),
            ..link
        },
        Err(refused) => refused.into(),
    }
}

/// The address's tail, read by the module it names.
fn typed(address: &Address) -> Result<DuckLink, Refused> {
    Ok(match address.module.as_str() {
        "pages" => {
            let page = PageAddress::try_from(address)?;
            DuckLink {
                page: page.page,
                block: page.block.unwrap_or_default(),
                ..DuckLink::of(DuckKind::Page)
            }
        }
        "chat" => {
            let message = MessageAddress::try_from(address)?;
            match message.seq {
                None => DuckLink {
                    channel: message.channel,
                    ..DuckLink::of(DuckKind::Channel)
                },
                Some(seq) => DuckLink {
                    channel: message.channel,
                    seq: counted(seq)?,
                    ..DuckLink::of(DuckKind::ChannelMessage)
                },
            }
        }
        "files" => DuckLink {
            path: format!("/{}", FileAddress::try_from(address)?.path.join("/")),
            ..DuckLink::of(DuckKind::Files)
        },
        "runs" => DuckLink {
            dispatch: RunAddress::try_from(address)?.digest,
            ..DuckLink::of(DuckKind::Run)
        },
        "identity" => DuckLink {
            account: AccountAddress::try_from(address)?.account.to_string(),
            ..DuckLink::of(DuckKind::Account)
        },
        // forge, the one module left (`Address::parse` refuses the rest): a
        // bare `<owner>/<repo>` is the repository, anything longer a locator.
        _ if address.path.len() == 2 => DuckLink {
            repo: ForgeRepoAddress::try_from(address)?.name(),
            ..DuckLink::of(DuckKind::ForgeRepo)
        },
        _ => {
            let locator = ForgeLocator::try_from(address)?;
            let repo = locator.repo.name();
            match locator.target {
                ForgeTarget::Item { number } => DuckLink {
                    repo,
                    number: counted(number)?,
                    ..DuckLink::of(DuckKind::ForgeItem)
                },
                ForgeTarget::Comment { number, seq } => DuckLink {
                    repo,
                    number: counted(number)?,
                    seq: counted(seq)?,
                    ..DuckLink::of(DuckKind::ForgeItem)
                },
                ForgeTarget::Blob { rev, path } => DuckLink {
                    repo,
                    rev,
                    path: path.join("/"),
                    ..DuckLink::of(DuckKind::ForgeBlob)
                },
            }
        }
    })
}

/// A number the open plane carries as `i64`; one past it names nothing.
fn counted(number: u64) -> Result<i64, Refused> {
    i64::try_from(number).map_err(|_| {
        Refused::new(
            "invalid_input",
            format!("{number} is past the largest number this app opens."),
        )
    })
}

/// The open plane's entry: the grammar, plus the one check the grammar cannot
/// make on its own. An address naming a network OTHER than the connected one
/// would resolve its repo name / page id / channel id against a store that is
/// not the address's own, so it opens nothing — the caller draws the refusal
/// [`foreign_network_error`] spells.
pub fn resolve_duck_link(url: String, connected_chain_id: String) -> DuckLink {
    let link = classify_duck_link(url);
    let connected = connected_chain_id.parse::<ChainId>().ok();
    let ours = link.chain.is_none() || link.chain == connected;
    match ours {
        true => link,
        false => DuckLink {
            kind: DuckKind::ForeignNetwork,
            ..link
        },
    }
}

/// The refusal for a link that belongs to another network: both networks by
/// name, because "this link does not open" without them is unactionable.
pub fn foreign_network_error(link: &DuckLink, connected_chain_id: String) -> String {
    let theirs = link.chain.as_ref().map(ToString::to_string);
    let here = match connected_chain_id.is_empty() {
        true => "no network".to_owned(),
        false => connected_chain_id,
    };
    format!(
        "this link belongs to network {} — this app is on {here}",
        theirs.unwrap_or_default()
    )
}

/// The `duck://` URL the OS launched this process with, or "" for a plain
/// start. `xdg-open 'duck://dognet-b5b6ea90/forge/core/app/1'` runs the
/// `Exec=` line of the desktop entry that claims `x-scheme-handler/duck`
/// (`app/packaging/dev.ducktape.app.desktop`), which passes the URL as `%u`.
///
/// Read once into state and PARKED, never opened here: the link addresses
/// objects in a network this process has not connected to yet, and the open
/// plane must know the connected chain id before it can tell an address of
/// its own from one of somebody else's.
pub fn startup_duck_url() -> String {
    std::env::args()
        .skip(1)
        .find(|argument| argument.starts_with("duck://"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HERE: &str = "dognet#b5b6ea90";
    const AT: &str = "duck://dognet-b5b6ea90";

    fn kind(url: &str) -> DuckKind {
        classify_duck_link(url.into()).kind
    }

    fn refusal(url: &str) -> String {
        let link = classify_duck_link(url.into());
        assert_eq!(link.kind, DuckKind::Unknown, "{url}");
        link.refusal
    }

    /// Every typed form, row by row, non-ASCII names included: the crate
    /// carries any segment percent-encoded in one spelling, and the app
    /// hands the decoded name on.
    #[test]
    fn every_typed_form_classifies_and_malformed_tails_say_why() {
        let page = classify_duck_link(format!("{AT}/pages/pg-1"));
        assert_eq!(
            (page.kind, page.page.as_str(), page.block.as_str()),
            (DuckKind::Page, "pg-1", "")
        );
        let block = classify_duck_link(format!("{AT}/pages/%ED%9A%8C%EC%9D%98/block/Blk_7"));
        assert_eq!(
            (block.kind, block.page.as_str(), block.block.as_str()),
            (DuckKind::Page, "회의", "Blk_7")
        );
        assert!(refusal(&format!("{AT}/pages/a/b")).contains("is neither"));

        let channel = classify_duck_link(format!("{AT}/chat/general"));
        assert_eq!(
            (channel.kind, channel.channel.as_str()),
            (DuckKind::Channel, "general")
        );
        let hidden = classify_duck_link(format!("{AT}/chat/forge%3Aducktape%3A58"));
        assert_eq!(
            (hidden.kind, hidden.channel.as_str()),
            (DuckKind::Channel, "forge:ducktape:58")
        );
        let message = classify_duck_link(format!("{AT}/chat/general/42"));
        assert_eq!((message.kind, message.seq), (DuckKind::ChannelMessage, 42));
        assert!(refusal(&format!("{AT}/chat/general/042")).contains("leading zero"));

        let file = classify_duck_link(format!(
            "{AT}/files/shared/attachments/u1/%EB%B3%B4%EA%B3%A0%EC%84%9C%20Final.pdf"
        ));
        assert_eq!(
            (file.kind, file.path.as_str()),
            (DuckKind::Files, "/shared/attachments/u1/보고서 Final.pdf")
        );
        assert_eq!(
            kind(&format!("{AT}/files/home/acct%3A7/Notes")),
            DuckKind::Files
        );
        assert!(refusal(&format!("{AT}/files/shared/e%CC%81")).contains("NFC"));
        assert_eq!(
            kind(&format!("{AT}/files/shared/../etc")),
            DuckKind::Unknown
        );

        let dispatch = "ab".repeat(32);
        let run = classify_duck_link(format!("{AT}/runs/{dispatch}"));
        assert_eq!(
            (run.kind, run.dispatch.as_str()),
            (DuckKind::Run, dispatch.as_str())
        );
        assert!(refusal(&format!("{AT}/runs/abc")).contains("64 lowercase hex"));

        let repo = classify_duck_link(format!("{AT}/forge/core/app"));
        assert_eq!(
            (repo.kind, repo.repo.as_str()),
            (DuckKind::ForgeRepo, "core/app")
        );
        let item = classify_duck_link(format!("{AT}/forge/core/app/58"));
        assert_eq!(
            (item.kind, item.repo.as_str(), item.number, item.seq),
            (DuckKind::ForgeItem, "core/app", 58, 0)
        );
        let comment = classify_duck_link(format!("{AT}/forge/core/app/58/comment/12"));
        assert_eq!(
            (comment.kind, comment.number, comment.seq),
            (DuckKind::ForgeItem, 58, 12)
        );
        let oid = "1".repeat(40);
        let blob = classify_duck_link(format!("{AT}/forge/core/app/blob/{oid}/docs/logo.png"));
        assert_eq!(
            (
                blob.kind,
                blob.repo.as_str(),
                blob.path.as_str(),
                blob.rev.as_str()
            ),
            (
                DuckKind::ForgeBlob,
                "core/app",
                "docs/logo.png",
                oid.as_str()
            )
        );
        assert!(refusal(&format!("{AT}/forge/core/app.git")).contains("Drop the `.git`"));
        assert_eq!(
            kind(&format!("{AT}/forge/core")),
            DuckKind::Unknown,
            "a flat name"
        );
        assert_eq!(
            kind(&format!("{AT}/forge/core/app/blob/main/README.md")),
            DuckKind::Unknown,
            "a branch name moves — a rev is an exact oid"
        );

        assert!(refusal(&format!("{AT}/memory/notes")).contains("names no ducktape module"));
        assert_eq!(kind(&format!("{AT}/pages/pg-1#blk")), DuckKind::Unknown);
        assert_eq!(kind("https://example.com/a.png"), DuckKind::Web);
        assert_eq!(kind("http://example.com"), DuckKind::Web);
        assert_eq!(
            refusal("mailto:a@b"),
            "this link names nothing the app can open"
        );
        assert_eq!(
            kind("./img/a.png"),
            DuckKind::Unknown,
            "the caller's to resolve"
        );
    }

    /// Links minted by the crate's own typed tails, as a view mints them,
    /// read back to the same thing — chat's account mention among them.
    #[test]
    fn minted_addresses_round_trip_through_the_classifier() {
        use duck_address::chat::MessageAddress;
        use duck_address::pages::PageAddress;
        use duck_address::runs::RunAddress;
        let chain: ChainId = HERE.parse().expect("a chain id");
        let page = PageAddress {
            page: "회의 notes".into(),
            block: Some("b1".into()),
        };
        let minted = page.address(chain.clone()).expect("mints").to_string();
        let link = resolve_duck_link(minted.clone(), HERE.into());
        assert_eq!(
            (link.page.as_str(), link.block.as_str()),
            ("회의 notes", "b1"),
            "{minted}"
        );
        let message = MessageAddress {
            channel: "general".into(),
            seq: Some(3),
        };
        let minted = message.address(chain.clone()).expect("mints").to_string();
        assert_eq!(resolve_duck_link(minted, HERE.into()).seq, 3);
        let run = RunAddress {
            digest: "cd".repeat(32),
        };
        let minted = run.address(chain.clone()).expect("mints").to_string();
        assert_eq!(resolve_duck_link(minted, HERE.into()).kind, DuckKind::Run);
        let repo = ForgeRepoAddress::from_name("core/app").expect("an owner/repo name");
        let minted = repo.address(chain.clone()).expect("mints").to_string();
        assert_eq!(resolve_duck_link(minted, HERE.into()).repo, "core/app");
        assert!(
            ForgeRepoAddress::from_name("ducktape").is_err(),
            "a flat name has no address"
        );
        let file = FileAddress {
            path: vec!["shared".into(), "보고서 Final.pdf".into()],
        };
        let minted = file.address(chain.clone()).expect("mints").to_string();
        assert_eq!(
            resolve_duck_link(minted, HERE.into()).path,
            "/shared/보고서 Final.pdf"
        );

        let minted = AccountAddress { account: 7 }
            .address(chain.clone())
            .expect("mints")
            .to_string();
        assert_eq!(minted, format!("{AT}/identity/7"));
        assert_eq!(
            minted,
            crate::interfaces::chat::client::duck_account_link(&chain, 7)
        );
        let account = resolve_duck_link(minted, HERE.into());
        assert_eq!(
            (account.kind, account.account.as_str(), account.chain),
            (DuckKind::Account, "7", Some(chain))
        );
        assert!(refusal(&format!("{AT}/identity/07")).contains("leading zero"));
        assert!(refusal(&format!("{AT}/identity/7/keys")).contains("is not one"));
    }

    /// THE OLD FORM (ruling Q4): every row of the module table this app
    /// used to read, with or without `?net=` as chat's writer spelled it, and
    /// the chain-less account mention, is refused with one sentence — never
    /// read as the module it names. Pasted or launched, `duck://account/<n>`
    /// is an old form like the rest.
    #[test]
    fn the_old_form_is_refused_with_the_sentence_not_misrouted() {
        let dispatch = "ab".repeat(32);
        let net = "?net=b5b6ea90";
        for old in [
            "duck://page/pg-1".to_owned(),
            "duck://page/pg-1#blk-7".into(),
            format!("duck://page/p1{net}"),
            "duck://files/shared/attachments/u1/doc.pdf".into(),
            "duck://forge/ducktape".into(),
            "duck://forge/ducktape/58#12".into(),
            format!("duck://forge/ducktape/58{net}"),
            "duck://forge/ducktape/blob/docs/logo.png".into(),
            format!("duck://forge/d/blob/a.png@{}{net}", "1".repeat(40)),
            "duck://channel/general".into(),
            format!("duck://channel/general{net}#42"),
            format!("duck://run/{dispatch}"),
            "duck://team.duck/index.html".into(),
            "duck://account/7".into(),
            "duck://".into(),
        ] {
            assert_eq!(refusal(&old), OLD_FORM, "{old}");
        }
        assert!(
            refusal(&format!("{AT}/pages/p1{net}")).contains("query"),
            "the new authority does not carry the old query either"
        );
    }

    /// The scope check: the authority against the connected chain id, as the
    /// crate's `ChainId` — the registry's `#` and the address's `-` are one
    /// pair.
    #[test]
    fn a_link_from_another_network_is_refused_not_resolved() {
        let mine = resolve_duck_link(format!("{AT}/forge/core/app/58"), HERE.into());
        assert_eq!((mine.kind, mine.number), (DuckKind::ForgeItem, 58));
        let theirs = resolve_duck_link(
            "duck://dognet-aaaaaaaa/forge/core/app/58".into(),
            HERE.into(),
        );
        assert_eq!(
            theirs.kind,
            DuckKind::ForeignNetwork,
            "the same repo name on another network is not this repo"
        );
        let relabelled =
            resolve_duck_link("duck://catnet-b5b6ea90/chat/general".into(), HERE.into());
        assert_eq!(
            relabelled.kind,
            DuckKind::ForeignNetwork,
            "the chain id whole"
        );
        let unjoined = resolve_duck_link(format!("{AT}/pages/p1"), String::new());
        assert_eq!(
            unjoined.kind,
            DuckKind::ForeignNetwork,
            "no connected chain id is no store to resolve against"
        );
        assert_eq!(
            resolve_duck_link("https://example.com".into(), HERE.into()).kind,
            DuckKind::Web,
            "a web link belongs to no network"
        );
        assert_eq!(
            resolve_duck_link(format!("{AT}/identity/7"), HERE.into()).account,
            "7"
        );
        assert_eq!(
            resolve_duck_link("duck://dognet-aaaaaaaa/identity/7".into(), HERE.into()).kind,
            DuckKind::ForeignNetwork,
            "account 7 on another network is not this network's account 7"
        );

        let refused = foreign_network_error(&theirs, HERE.into());
        assert!(
            refused.contains("dognet#aaaaaaaa") && refused.contains(HERE),
            "{refused}"
        );
        assert!(
            foreign_network_error(&theirs, String::new()).contains("no network"),
            "an unconnected app still names where it is"
        );
    }
}
