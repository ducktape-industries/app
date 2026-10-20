//! A release record: the signed channel file byte for byte, and for each artifact the network blob
//! that holds it and the check that made it. Only the signed manifest is trusted: a blob id locates
//! an artifact's bytes, and the manifest's sha256 and size are what those bytes must be.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::sha::Sha;
use crate::verify::{Refusal, SignedManifest, TrustedKeys, VerifiedManifest, verify_manifest};

pub const RECORD: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRecord {
    pub record: u32,
    pub channel_file: String,
    pub artifacts: BTreeMap<String, RecordedArtifact>,
    pub source: Source,
    pub inputs: Vec<Input>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedArtifact {
    pub sha256: Sha,
    pub size: u64,
    pub blob: String,
    pub check: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub repo: String,
    pub commit: String,
    pub check: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub job: String,
    pub check: u64,
    pub path: String,
    pub blob: String,
    pub sha256: Sha,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordRefusal {
    Manifest(Refusal),
    RecordUnsupported,
    ArtifactMismatch,
    BlobMismatch,
}

impl fmt::Display for RecordRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordRefusal::Manifest(refusal) => refusal.fmt(f),
            RecordRefusal::RecordUnsupported => f.write_str("record_unsupported"),
            RecordRefusal::ArtifactMismatch => f.write_str("record_artifact_mismatch"),
            RecordRefusal::BlobMismatch => f.write_str("record_blob_mismatch"),
        }
    }
}

impl std::error::Error for RecordRefusal {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRecord {
    pub verified: VerifiedManifest,
    pub record: ReleaseRecord,
}

pub fn verify_record(
    record: &ReleaseRecord,
    channel: &str,
    keys: &TrustedKeys,
) -> Result<VerifiedRecord, RecordRefusal> {
    if record.record != RECORD {
        return Err(RecordRefusal::RecordUnsupported);
    }
    let signed = SignedManifest::from_file(record.channel_file.as_bytes())
        .map_err(RecordRefusal::Manifest)?;
    let verified = verify_manifest(&signed, channel, keys).map_err(RecordRefusal::Manifest)?;
    let signed_artifacts = &verified.manifest.artifacts;
    let same_platforms = signed_artifacts.keys().eq(record.artifacts.keys());
    let same_bytes = record.artifacts.iter().all(|(platform, recorded)| {
        signed_artifacts
            .get(platform)
            .is_some_and(|signed| signed.sha256 == recorded.sha256 && signed.size == recorded.size)
    });
    if !same_platforms || !same_bytes {
        return Err(RecordRefusal::ArtifactMismatch);
    }
    let located = record
        .artifacts
        .values()
        .all(|recorded| blob_digest(&recorded.blob).is_some());
    if !located {
        return Err(RecordRefusal::BlobMismatch);
    }
    Ok(VerifiedRecord {
        verified,
        record: record.clone(),
    })
}

impl VerifiedRecord {
    pub fn artifact_bytes<'a>(
        &self,
        platform: &str,
        framed: &'a [u8],
    ) -> Result<&'a [u8], RecordRefusal> {
        let signed = self
            .verified
            .manifest
            .artifacts
            .get(platform)
            .ok_or(RecordRefusal::Manifest(Refusal::NoArtifactForPlatform))?;
        let recorded = self
            .record
            .artifacts
            .get(platform)
            .ok_or(RecordRefusal::ArtifactMismatch)?;
        let digest = blob_digest(&recorded.blob).ok_or(RecordRefusal::BlobMismatch)?;
        if Sha::digest(framed) != digest {
            return Err(RecordRefusal::BlobMismatch);
        }
        let header_end = framed
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(RecordRefusal::BlobMismatch)?;
        let body = &framed[header_end + 1..];
        let is_signed_artifact =
            body.len() as u64 == signed.size && Sha::digest(body) == signed.sha256;
        if !is_signed_artifact {
            return Err(RecordRefusal::BlobMismatch);
        }
        Ok(body)
    }
}

fn blob_digest(blob: &str) -> Option<Sha> {
    blob.strip_prefix("sha256:")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::testkit::sample;
    use crate::verify::testkit::{CHANNEL, fixture};

    const PLATFORM: &str = "linux-x86_64";

    type Edit = fn(&mut ReleaseRecord);

    fn framed(body: &[u8]) -> Vec<u8> {
        [format!("output {}\0", body.len()).as_bytes(), body].concat()
    }

    fn blob_of(body: &[u8]) -> String {
        format!("sha256:{}", Sha::digest(&framed(body)))
    }

    fn record_of(body: &[u8]) -> (ReleaseRecord, TrustedKeys) {
        let mut manifest = sample(17, PLATFORM, Sha::digest(body));
        manifest.artifacts.get_mut(PLATFORM).unwrap().size = body.len() as u64;
        let f = fixture(manifest.sealed());
        let artifact = RecordedArtifact {
            sha256: Sha::digest(body),
            size: body.len() as u64,
            blob: blob_of(body),
            check: 1340,
        };
        let record = ReleaseRecord {
            record: RECORD,
            channel_file: f.signed.file(),
            artifacts: BTreeMap::from([(PLATFORM.to_string(), artifact)]),
            source: Source {
                repo: "app".into(),
                commit: "c0ffee".repeat(6) + "c0ff",
                check: 1756,
            },
            inputs: Vec::new(),
        };
        (record, f.keys)
    }

    #[test]
    fn a_record_verifies_and_its_blob_yields_the_signed_bytes() {
        let (record, keys) = record_of(b"archive");
        let json = serde_json::to_string(&record).unwrap();
        let back: ReleaseRecord = serde_json::from_str(&json).unwrap();
        let verified = verify_record(&back, CHANNEL, &keys).unwrap();
        assert_eq!(verified.verified.manifest.sequence, 17);
        let bytes = framed(b"archive");
        assert_eq!(
            verified.artifact_bytes(PLATFORM, &bytes).unwrap(),
            b"archive"
        );
    }

    #[test]
    fn a_record_that_disagrees_with_its_signed_manifest_is_refused() {
        let (record, keys) = record_of(b"archive");
        let edits: [(Edit, RecordRefusal); 7] = [
            (
                |r| r.artifacts.get_mut(PLATFORM).unwrap().sha256 = Sha::digest(b"other"),
                RecordRefusal::ArtifactMismatch,
            ),
            (
                |r| r.artifacts.get_mut(PLATFORM).unwrap().size += 1,
                RecordRefusal::ArtifactMismatch,
            ),
            (
                |r| {
                    let extra = r.artifacts[PLATFORM].clone();
                    r.artifacts.insert("macos-aarch64".into(), extra);
                },
                RecordRefusal::ArtifactMismatch,
            ),
            (|r| r.artifacts.clear(), RecordRefusal::ArtifactMismatch),
            (
                |r| {
                    r.artifacts.get_mut(PLATFORM).unwrap().blob = "https://example.invalid/a".into()
                },
                RecordRefusal::BlobMismatch,
            ),
            (|r| r.record = 2, RecordRefusal::RecordUnsupported),
            (
                |r| r.channel_file = r.channel_file.replacen("17", "18", 1),
                RecordRefusal::Manifest(Refusal::BadSignature),
            ),
        ];
        for (edit, refusal) in edits {
            let mut edited = record.clone();
            edit(&mut edited);
            assert_eq!(verify_record(&edited, CHANNEL, &keys).unwrap_err(), refusal);
        }
    }

    #[test]
    fn a_blob_that_is_not_the_signed_artifact_is_refused() {
        let (record, keys) = record_of(b"archive");
        let verified = verify_record(&record, CHANNEL, &keys).unwrap();
        let wrong = framed(b"evil");
        assert_eq!(
            verified.artifact_bytes(PLATFORM, &wrong).unwrap_err(),
            RecordRefusal::BlobMismatch,
            "bytes that are not the blob its id names"
        );
        let mut elsewhere = record.clone();
        elsewhere.artifacts.get_mut(PLATFORM).unwrap().blob = blob_of(b"evil");
        let verified = verify_record(&elsewhere, CHANNEL, &keys).unwrap();
        assert_eq!(
            verified.artifact_bytes(PLATFORM, &wrong).unwrap_err(),
            RecordRefusal::BlobMismatch,
            "a whole blob that is not the signed artifact"
        );
        let mut unframed = record.clone();
        unframed.artifacts.get_mut(PLATFORM).unwrap().blob =
            format!("sha256:{}", Sha::digest(b"archive"));
        let verified = verify_record(&unframed, CHANNEL, &keys).unwrap();
        assert_eq!(
            verified.artifact_bytes(PLATFORM, b"archive").unwrap_err(),
            RecordRefusal::BlobMismatch,
            "a blob with no frame"
        );
        assert_eq!(
            verified.artifact_bytes("macos-aarch64", b"").unwrap_err(),
            RecordRefusal::Manifest(Refusal::NoArtifactForPlatform)
        );
    }

    #[test]
    fn refusals_display_snake_case() {
        assert_eq!(
            RecordRefusal::RecordUnsupported.to_string(),
            "record_unsupported"
        );
        assert_eq!(
            RecordRefusal::ArtifactMismatch.to_string(),
            "record_artifact_mismatch"
        );
        assert_eq!(
            RecordRefusal::BlobMismatch.to_string(),
            "record_blob_mismatch"
        );
        assert_eq!(
            RecordRefusal::Manifest(Refusal::BadSignature).to_string(),
            "bad_signature"
        );
    }
}
