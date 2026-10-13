//! `publish-release`: the release job's signer.
//!
//! Given one archive per platform, it names each by its own sha256
//! ([`app_update::layout::archive_name`]), writes the sealed manifest of
//! `--channel` (`stable.json` or `nightly.json`), signs it with the seed in
//! `RELEASE_SIGNING_KEY` and checks the pair the way the app does before it
//! says done. Everything lands in `--out`, ready to upload: the archives
//! first, then the manifest, then its `.sig`. `--sequence` is the built
//! commit's committer time (unix seconds), the order both channels share.
//!
//! The seed was stored without a round trip, so BEFORE anything is signed its
//! public half is derived and compared with the key the app pins
//! ([`app_update::RELEASE_KEY`]); a mismatch signs nothing. `--expect-key` is
//! for a dry run with a throwaway seed only: the release job never passes it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use app_update::layout::Channel;
use app_update::release::signing_key_from_hex;
use app_update::{
    Artifact, Manifest, PublicKey, RELEASE_KEY, Release, SCHEMA, Sha, Signature, SignedManifest,
    TrustedKeys, layout, verify_manifest,
};

const USAGE: &str = "usage: RELEASE_SIGNING_KEY=<64 hex> publish-release --channel stable|nightly \
--sequence <committer time> --display TEXT \
--published-at TIME --out DIR --archive <os>-<arch>=PATH [--archive ...] [--notes-url URL] \
[--expect-key HEX]";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    channel: Channel,
    sequence: u64,
    display: String,
    published_at: String,
    notes_url: String,
    out: PathBuf,
    archives: Vec<(String, PathBuf)>,
    expect: PublicKey,
}

fn main() -> ExitCode {
    let published = parse(std::env::args().skip(1)).and_then(|args| {
        let seed = std::env::var("RELEASE_SIGNING_KEY")
            .map_err(|_| "RELEASE_SIGNING_KEY is not set".to_string())?;
        publish(&args, &seed)
    });
    match published {
        Ok(summary) => {
            print!("{summary}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("publish-release: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let (mut channel, mut sequence, mut display, mut published_at, mut out) =
        (None, None, None, None, None);
    let (mut notes_url, mut archives, mut expect) = (String::new(), Vec::new(), RELEASE_KEY);
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--channel" => {
                channel = Some(Channel::of(&value).ok_or("--channel is stable or nightly")?);
            }
            "--sequence" => {
                sequence = Some(value.parse().map_err(|_| "--sequence takes a number")?);
            }
            "--display" => display = Some(value),
            "--published-at" => published_at = Some(value),
            "--notes-url" => notes_url = value,
            "--out" => out = Some(PathBuf::from(value)),
            "--archive" => {
                let (platform, path) = value
                    .split_once('=')
                    .filter(|(platform, _)| platform.matches('-').count() == 1)
                    .ok_or("--archive takes <os>-<arch>=PATH")?;
                archives.push((platform.to_string(), PathBuf::from(path)));
            }
            "--expect-key" => {
                expect = value.parse().map_err(|_| "--expect-key takes 64 hex")?;
            }
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    if archives.is_empty() {
        return Err("no --archive".into());
    }
    Ok(Args {
        channel: channel.ok_or("--channel is required")?,
        sequence: sequence.ok_or("--sequence is required")?,
        display: display.ok_or("--display is required")?,
        published_at: published_at.ok_or("--published-at is required")?,
        notes_url,
        out: out.ok_or("--out is required")?,
        archives,
        expect,
    })
}

/// Name the archives, seal and sign the manifest, check it as the app
/// would, and write all of it under `args.out`. The summary names what to
/// upload, in order.
fn publish(args: &Args, seed: &str) -> Result<String, String> {
    let signer = signing_key_from_hex(seed)
        .ok_or("RELEASE_SIGNING_KEY is not 64 hex characters (a 32-byte ed25519 seed)")?;
    let derived = PublicKey::of(&signer);
    if derived != args.expect {
        return Err(format!(
            "the seed's public key {derived} is not the pinned {}: nothing was signed",
            args.expect
        ));
    }
    let io = |what: &str, error: std::io::Error| format!("{what}: {error}");
    std::fs::create_dir_all(&args.out).map_err(|error| io("--out", error))?;
    let mut summary = String::new();
    let mut artifacts = BTreeMap::new();
    for (platform, path) in &args.archives {
        let bytes = std::fs::read(path).map_err(|error| io(&path.display().to_string(), error))?;
        let sha = Sha::digest(&bytes);
        let name = layout::archive_name(&sha, platform);
        std::fs::write(args.out.join(&name), &bytes).map_err(|error| io(&name, error))?;
        summary.push_str(&format!("{name}\n"));
        let artifact = Artifact {
            sha256: sha,
            size: bytes.len() as u64,
        };
        if artifacts.insert(platform.clone(), artifact).is_some() {
            return Err(format!("two archives for {platform}"));
        }
    }
    let manifest = Manifest {
        schema: SCHEMA,
        channel: args.channel.name().into(),
        sequence: args.sequence,
        published_at: args.published_at.clone(),
        release: Release {
            sha256_id: Sha::ZERO,
            display: args.display.clone(),
            notes_url: args.notes_url.clone(),
        },
        artifacts,
        successor_key: None,
    }
    .sealed();
    let mut bytes = serde_json::to_vec_pretty(&manifest).expect("a Manifest serializes");
    bytes.push(b'\n');
    let signature = Signature::sign(&signer, &bytes).encoded();
    // the app's own check, on the two files exactly as they will be served
    let keys = TrustedKeys {
        pinned: args.expect,
        successor: None,
    };
    SignedManifest::from_files(bytes.clone(), &signature)
        .and_then(|signed| verify_manifest(&signed, args.channel.name(), &keys))
        .map_err(|refusal| format!("the signed manifest does not verify: {refusal}"))?;
    for (name, text) in [
        (args.channel.manifest(), bytes),
        (args.channel.signature(), signature.into()),
    ] {
        std::fs::write(args.out.join(&name), text).map_err(|error| io(&name, error))?;
        summary.push_str(&format!("{name}\n"));
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_update::Platform;

    fn args(out: PathBuf, archive: PathBuf, expect: PublicKey) -> Args {
        Args {
            channel: Channel::Nightly,
            sequence: 7,
            display: "0.1.0+abc1234".into(),
            published_at: "2026-10-13T00:00:00Z".into(),
            notes_url: String::new(),
            out,
            archives: vec![(Platform::HOST.key(), archive)],
            expect,
        }
    }

    #[test]
    fn the_flags_parse_and_a_missing_one_is_named() {
        let parsed = parse(
            [
                "--channel",
                "nightly",
                "--sequence",
                "7",
                "--display",
                "d",
                "--published-at",
                "t",
                "--out",
                "/o",
                "--archive",
                "linux-x86_64=/a.tar.gz",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(parsed.expect, RELEASE_KEY, "the pinned key unless told");
        assert_eq!(parsed.channel, Channel::Nightly);
        assert_eq!(
            parsed.archives,
            vec![("linux-x86_64".into(), PathBuf::from("/a.tar.gz"))]
        );
        let missing = parse(["--sequence", "7"].map(String::from)).unwrap_err();
        assert_eq!(missing, "no --archive");
        let bad = parse(["--archive", "linux=/a"].map(String::from)).unwrap_err();
        assert!(bad.contains("<os>-<arch>"), "{bad}");
        let bad = parse(["--channel", "beta"].map(String::from)).unwrap_err();
        assert!(bad.contains("stable or nightly"), "{bad}");
        let unnamed = parse(["--archive", "linux-x86_64=/a"].map(String::from)).unwrap_err();
        assert_eq!(unnamed, "--channel is required");
    }

    /// A seed whose public half is not the pinned key signs nothing and
    /// writes nothing.
    #[test]
    fn a_seed_that_is_not_the_pinned_keys_signs_nothing() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("a.tar.gz");
        std::fs::write(&archive, b"archive").unwrap();
        let out = root.path().join("out");
        let refused = publish(&args(out.clone(), archive, RELEASE_KEY), &"07".repeat(32));
        assert!(refused.unwrap_err().contains("nothing was signed"));
        assert!(!out.exists());
    }

    /// The published pair is the channel's, verifies under the key it was
    /// signed for, and names the archive by its own sha256 and size.
    #[test]
    fn a_publish_writes_an_archive_named_by_its_sha_and_a_pair_the_app_accepts() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("a.tar.gz");
        std::fs::write(&archive, b"archive").unwrap();
        let out = root.path().join("out");
        let seed = "07".repeat(32);
        let key = PublicKey::of(&signing_key_from_hex(&seed).unwrap());
        let summary = publish(&args(out.clone(), archive, key), &seed).unwrap();
        let name = layout::archive_name(&Sha::digest(b"archive"), &Platform::HOST.key());
        assert_eq!(summary, format!("{name}\nnightly.json\nnightly.json.sig\n"));
        assert_eq!(std::fs::read(out.join(&name)).unwrap(), b"archive");
        let signed = SignedManifest::from_files(
            std::fs::read(out.join("nightly.json")).unwrap(),
            &std::fs::read_to_string(out.join("nightly.json.sig")).unwrap(),
        )
        .unwrap();
        let keys = TrustedKeys {
            pinned: key,
            successor: None,
        };
        assert_eq!(
            verify_manifest(&signed, "stable", &keys).unwrap_err(),
            app_update::Refusal::ChannelMismatch
        );
        let verified = verify_manifest(&signed, "nightly", &keys).unwrap();
        let artifact = verified.manifest.artifact_for(Platform::HOST).unwrap();
        assert_eq!(
            (artifact.sha256, artifact.size),
            (Sha::digest(b"archive"), 7)
        );
        assert_eq!(verified.manifest.sequence, 7);
    }
}
