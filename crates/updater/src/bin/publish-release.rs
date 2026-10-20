//! `publish-release`: the release job's signer.
//!
//! Given one archive per platform, it names each by its own sha256
//! ([`ducktape_updater::layout::archive_name`]), seals the manifest of
//! `--channel`, signs it with the seed in `RELEASE_SIGNING_KEY`, writes both
//! into the channel's one file (`stable.json` or `nightly.json`) and checks
//! that file the way the app does before it says done. Everything lands in
//! `--out`, ready to upload: the archives first, then the channel file.
//! `--sequence` is the built commit's committer time (unix seconds), the
//! order both channels share.
//!
//! The seed was stored without a round trip, so BEFORE anything is signed its
//! public half is derived and compared with the key the app pins
//! ([`ducktape_updater::RELEASE_KEY`]); a mismatch signs nothing. `--expect-key` is
//! for a dry run with a throwaway seed only: the release job never passes it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use ducktape_updater::layout::Channel;
use ducktape_updater::release::signing_key_from_hex;
use ducktape_updater::{
    Artifact, Input, Manifest, PublicKey, RECORD, RELEASE_KEY, RecordedArtifact, Release,
    ReleaseRecord, SCHEMA, Sha, Signature, SignedManifest, Source, TrustedKeys, layout,
    verify_manifest, verify_record,
};

const USAGE: &str = "usage: RELEASE_SIGNING_KEY=<64 hex> publish-release --channel stable|nightly \
--sequence <committer time> --display TEXT \
--published-at TIME --out DIR --archive <os>-<arch>=PATH [--archive ...] [--notes-url URL] \
[--expect-key HEX] [--record FILES_ROOT, as the signer's release-manifest tool: SIGNER_INPUTS, \
SIGNER_REPO, SIGNER_COMMIT and SIGNER_CHECK set, and --published-at the commit's time]";

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
    record: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Handed {
    inputs: Vec<Input>,
    repo: String,
    commit: String,
    check: u64,
}

fn main() -> ExitCode {
    let published = parse(std::env::args().skip(1)).and_then(|args| {
        let seed = std::env::var("RELEASE_SIGNING_KEY")
            .map_err(|_| "RELEASE_SIGNING_KEY is not set".to_string())?;
        let handed = match args.record {
            Some(_) => Some(handed()?),
            None => None,
        };
        publish(&args, &seed, handed.as_ref())
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

fn handed() -> Result<Handed, String> {
    let var = |name: &str| {
        std::env::var(name).map_err(|_| format!("--record needs {name}, which the signer sets"))
    };
    let path = var("SIGNER_INPUTS")?;
    let text = std::fs::read(&path).map_err(|error| format!("{path}: {error}"))?;
    let inputs = serde_json::from_slice(&text).map_err(|error| format!("{path}: {error}"))?;
    let check = var("SIGNER_CHECK")?;
    Ok(Handed {
        inputs,
        repo: var("SIGNER_REPO")?,
        commit: var("SIGNER_COMMIT")?,
        check: check
            .parse()
            .map_err(|_| format!("SIGNER_CHECK is {check:?}, not a check id"))?,
    })
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let (mut channel, mut sequence, mut display, mut published_at, mut out) =
        (None, None, None, None, None);
    let (mut notes_url, mut archives, mut expect) = (String::new(), Vec::new(), RELEASE_KEY);
    let mut record = None;
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
            "--record" => record = Some(value),
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
        record,
    })
}

fn publish(args: &Args, seed: &str, handed: Option<&Handed>) -> Result<String, String> {
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
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("a Manifest serializes");
    let signature = Signature::sign(&signer, &manifest_bytes);
    let file = SignedManifest {
        manifest_bytes,
        signature,
    }
    .file();
    let keys = TrustedKeys {
        pinned: args.expect,
        successor: None,
    };
    SignedManifest::from_file(file.as_bytes())
        .and_then(|signed| verify_manifest(&signed, args.channel.name(), &keys))
        .map_err(|refusal| format!("the signed manifest does not verify: {refusal}"))?;
    let name = args.channel.file();
    std::fs::write(args.out.join(&name), &file).map_err(|error| io(&name, error))?;
    summary.push_str(&format!("{name}\n"));
    if let (Some(root), Some(handed)) = (&args.record, handed) {
        record(args, root, handed, &manifest, file, &keys)?;
    }
    Ok(summary)
}

fn record(
    args: &Args,
    root: &str,
    handed: &Handed,
    manifest: &Manifest,
    channel_file: String,
    keys: &TrustedKeys,
) -> Result<(), String> {
    let mut artifacts = BTreeMap::new();
    for (platform, artifact) in &manifest.artifacts {
        let input = handed
            .inputs
            .iter()
            .find(|input| input.sha256 == artifact.sha256)
            .ok_or_else(|| {
                format!(
                    "{platform}'s archive (sha256 {}) is no input the signer handed over: the record names each artifact by its blob",
                    artifact.sha256
                )
            })?;
        let recorded = RecordedArtifact {
            sha256: artifact.sha256,
            size: artifact.size,
            blob: input.blob.clone(),
            check: input.check,
        };
        artifacts.insert(platform.clone(), recorded);
    }
    let record = ReleaseRecord {
        record: RECORD,
        channel_file,
        artifacts,
        source: Source {
            repo: handed.repo.clone(),
            commit: handed.commit.clone(),
            check: handed.check,
        },
        inputs: handed.inputs.clone(),
    };
    verify_record(&record, args.channel.name(), keys)
        .map_err(|refusal| format!("the record does not verify: {refusal}"))?;
    let mut text = serde_json::to_string_pretty(&record).expect("a record serializes");
    text.push('\n');
    let io = |what: &str, error: std::io::Error| format!("{what}: {error}");
    std::fs::write(args.out.join("record.json"), &text)
        .map_err(|error| io("record.json", error))?;
    let root = root.trim_end_matches('/');
    let out = args
        .out
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("--out names a folder")?;
    let files: Vec<serde_json::Value> = args
        .archives
        .iter()
        .map(|(platform, _)| {
            let sha = manifest.artifacts[platform].sha256;
            let name = layout::archive_name(&sha, platform);
            serde_json::json!({"path": format!("{root}/{sha}/{name}"), "from": format!("{out}/{name}")})
        })
        .collect();
    let tag = serde_json::json!({
        "name": format!("release/{}/{}", args.channel.name(), args.sequence),
        "message": text,
        "time": args.sequence,
    });
    let publish = serde_json::json!({"files": files, "tag": tag}).to_string();
    std::fs::write(args.out.join("publish.json"), publish)
        .map_err(|error| io("publish.json", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ducktape_updater::Platform;

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
            record: None,
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

    #[test]
    fn a_seed_that_is_not_the_pinned_keys_signs_nothing() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("a.tar.gz");
        std::fs::write(&archive, b"archive").unwrap();
        let out = root.path().join("out");
        let refused = publish(
            &args(out.clone(), archive, RELEASE_KEY),
            &"07".repeat(32),
            None,
        );
        assert!(refused.unwrap_err().contains("nothing was signed"));
        assert!(!out.exists());
    }

    #[test]
    fn a_publish_writes_an_archive_named_by_its_sha_and_a_pair_the_app_accepts() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("a.tar.gz");
        std::fs::write(&archive, b"archive").unwrap();
        let out = root.path().join("out");
        let seed = "07".repeat(32);
        let key = PublicKey::of(&signing_key_from_hex(&seed).unwrap());
        let summary = publish(&args(out.clone(), archive, key), &seed, None).unwrap();
        let name = layout::archive_name(&Sha::digest(b"archive"), &Platform::HOST.key());
        assert_eq!(summary, format!("{name}\nnightly.json\n"));
        assert_eq!(std::fs::read(out.join(&name)).unwrap(), b"archive");
        let signed =
            SignedManifest::from_file(&std::fs::read(out.join("nightly.json")).unwrap()).unwrap();
        let keys = TrustedKeys {
            pinned: key,
            successor: None,
        };
        assert_eq!(
            verify_manifest(&signed, "stable", &keys).unwrap_err(),
            ducktape_updater::Refusal::ChannelMismatch
        );
        let verified = verify_manifest(&signed, "nightly", &keys).unwrap();
        let artifact = verified.manifest.artifact_for(Platform::HOST).unwrap();
        assert_eq!(
            (artifact.sha256, artifact.size),
            (Sha::digest(b"archive"), 7)
        );
        assert_eq!(verified.manifest.sequence, 7);
    }

    #[test]
    fn a_record_names_each_archive_by_the_blob_the_signer_handed_over_and_asks_for_its_tag() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("a.tar.gz");
        std::fs::write(&archive, b"archive").unwrap();
        let out = root.path().join("out");
        let seed = "07".repeat(32);
        let key = PublicKey::of(&signing_key_from_hex(&seed).unwrap());
        let sha = Sha::digest(b"archive");
        let input = Input {
            job: "linux".into(),
            check: 1340,
            path: "dist/a.tar.gz".into(),
            blob: format!("sha256:{}", "ab".repeat(32)),
            sha256: sha,
        };
        let handed = Handed {
            inputs: vec![input.clone()],
            repo: "app".into(),
            commit: "c0ffee".repeat(6) + "c0ff",
            check: 1756,
        };
        let mut asked = args(out.clone(), archive.clone(), key);
        asked.record = Some("/shared/releases/app/".into());
        let summary = publish(&asked, &seed, Some(&handed)).unwrap();
        let name = layout::archive_name(&sha, &Platform::HOST.key());
        assert_eq!(
            summary,
            format!("{name}\nnightly.json\n"),
            "the R2 upload order is unchanged"
        );
        let text = std::fs::read_to_string(out.join("record.json")).unwrap();
        let record: ReleaseRecord = serde_json::from_str(&text).unwrap();
        let keys = TrustedKeys {
            pinned: key,
            successor: None,
        };
        let verified = verify_record(&record, "nightly", &keys).unwrap();
        assert_eq!(
            verified.record.artifacts[&Platform::HOST.key()].blob,
            input.blob
        );
        assert_eq!((record.source.check, record.inputs), (1756, vec![input]));
        assert_eq!(
            record.channel_file,
            std::fs::read_to_string(out.join("nightly.json")).unwrap(),
            "the channel file R2 serves, byte for byte"
        );
        let asked_for: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("publish.json")).unwrap()).unwrap();
        let wanted = serde_json::json!({
            "files": [{"path": format!("/shared/releases/app/{sha}/{name}"), "from": format!("out/{name}")}],
            "tag": {"name": "release/nightly/7", "message": text, "time": 7},
        });
        assert_eq!(asked_for, wanted);
        let stranger = Handed {
            inputs: Vec::new(),
            ..handed
        };
        let refused = publish(&asked, &seed, Some(&stranger)).unwrap_err();
        assert!(
            refused.contains("is no input the signer handed over"),
            "{refused}"
        );
    }
}
