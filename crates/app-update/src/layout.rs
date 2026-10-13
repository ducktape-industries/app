//! Where a release is published: a public bucket served over HTTPS at
//! [`BASE_URL`]. Fixed: the publisher writes these names and the app reads
//! them; a manifest names no location, only content (a sha256 and a size),
//! and an archive's name is derived from that content and the platform.
//!
//! ```text
//! https://dl.ducktape.industries/stable.json                            a channel's manifest
//! https://dl.ducktape.industries/stable.json.sig                        its signature
//! https://dl.ducktape.industries/nightly.json(.sig)                     the other channel's
//! https://dl.ducktape.industries/Ducktape-<sha256>-<os>-<arch>.tar.gz   one archive per platform
//! ```
//!
//! An archive's name carries its whole sha256, so a name never changes
//! bytes: the bucket serves archives as immutable and only the small
//! manifest files under a short cache.

use crate::sha::Sha;

/// The bucket every release file is published under.
pub const BASE_URL: &str = "https://dl.ducktape.industries/";

/// A release channel: its own signed manifest under [`BASE_URL`], signed by
/// the same key as the other. Stable is `main`, nightly is `dev` once a day.
/// Sequences are ONE order across both (a release's sequence is its commit's
/// committer time), so "newer" means the same thing whichever channel a
/// release came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    #[default]
    Stable,
    Nightly,
}

impl Channel {
    pub const ALL: [Channel; 2] = [Channel::Stable, Channel::Nightly];

    /// The manifest's `channel` field and its file's stem.
    pub fn name(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Nightly => "nightly",
        }
    }

    pub fn of(name: &str) -> Option<Channel> {
        Channel::ALL
            .into_iter()
            .find(|channel| channel.name() == name)
    }

    /// The manifest's name under [`BASE_URL`].
    pub fn manifest(self) -> String {
        format!("{}.json", self.name())
    }

    /// The signature's name under [`BASE_URL`].
    pub fn signature(self) -> String {
        format!("{}.json.sig", self.name())
    }
}

/// `Ducktape-<sha256>-<os>-<arch>.tar.gz`: the archive's name, from its own
/// sha256 and the platform key it runs on ([`crate::Platform::key`]).
pub fn archive_name(sha: &Sha, platform_key: &str) -> String {
    format!("Ducktape-{sha}-{platform_key}.tar.gz")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_have_the_fixed_shape() {
        assert_eq!(Channel::Stable.manifest(), "stable.json");
        assert_eq!(Channel::Nightly.signature(), "nightly.json.sig");
        assert_eq!(Channel::of("nightly"), Some(Channel::Nightly));
        assert_eq!(Channel::of("beta"), None);
        assert_eq!(Channel::default(), Channel::Stable);
        assert!(BASE_URL.starts_with("https://") && BASE_URL.ends_with('/'));
        let sha = Sha::digest(b"archive");
        let platform = crate::manifest::Platform {
            os: "macos",
            arch: "aarch64",
        };
        assert_eq!(
            archive_name(&sha, &platform.key()),
            format!("Ducktape-{sha}-macos-aarch64.tar.gz")
        );
    }
}
