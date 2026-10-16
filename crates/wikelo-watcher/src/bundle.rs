//! Dataset bundles: a zip with `manifest.json` (every file with its SHA-256, who
//! produced it, the reproducible `contentHash` of the dataset and the extractor's
//! provenance), the channel's `wikelo-data.json`, the images extracted from the game
//! (`img/`), and optionally player pictures (`custom/`), 3D renders (`render/`) and
//! their credits (`credits.json`).
//!
//! The watcher builds them on a player's PC without any signature: trust comes from
//! the server validating the content in quarantine and from the hash being
//! reproducible by anyone with the game. A server export (instance to instance)
//! may add `manifest.sig` (Ed25519 over the manifest bytes), verified when present.

use std::collections::HashSet;
use std::io::{Cursor, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

use crate::sign;

pub const DATASET_FILE: &str = "wikelo-data.json";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const SIGNATURE_FILE: &str = "manifest.sig";
pub const CREDITS_FILE: &str = "credits.json";
/// Format 1 bundles were always signed; format 2 adds `contentHash`/`extractor` and
/// makes the signature optional. Importers accept both.
pub const MANIFEST_FORMAT: u32 = 2;
/// Extensions a player picture may have.
pub const CUSTOM_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: u32,
    pub channel: String,
    /// Build version (`4.10.197.2878`).
    pub version: String,
    /// Short label (`4.10.2`).
    pub label: String,
    pub created_at: String,
    pub author: Author,
    pub files: Vec<ManifestFile>,
    /// `wikelo_core::canonical_hash` of the dataset, as computed by the producer. The
    /// server recomputes it and never trusts this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// Extractor build and game-file hashes (copied from the dataset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor: Option<wikelo_core::ExtractorInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Author {
    pub name: String,
    pub handle: String,
    /// Ed25519 public key, base64; only when the bundle is signed (server export).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// `watcher`, `local` or `server`.
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Img,
    Custom,
    Render,
}

/// A zip entry name we accept: `img/x.png|svg`, `custom/x.png|jpg|jpeg|webp`, `render/x.png`.
pub fn safe_entry(name: &str) -> Option<(EntryKind, String)> {
    let p = Path::new(name);
    let mut comps = p.components();
    let dir = match comps.next() {
        Some(Component::Normal(d)) => d.to_str()?,
        _ => return None,
    };
    let file = match (comps.next(), comps.next()) {
        (Some(Component::Normal(f)), None) => f.to_str()?,
        _ => return None,
    };
    if name.contains('\\') || file.contains("..") || file.len() > 200 || !file.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return None;
    }
    let ext = Path::new(file).extension()?.to_str()?.to_ascii_lowercase();
    let kind = match dir {
        "img" if matches!(ext.as_str(), "png" | "svg") => EntryKind::Img,
        "custom" if CUSTOM_EXTS.contains(&ext.as_str()) => EntryKind::Custom,
        "render" if ext == "png" => EntryKind::Render,
        _ => return None,
    };
    Some((kind, file.to_string()))
}

/// `wikelo-LIVE-4.10.2.zip`
pub fn file_name(channel: &str, label: &str) -> String {
    format!("wikelo-{}-{}.zip", channel.to_ascii_uppercase(), label)
}

/// Files of `dir` as `(prefix/name, bytes)`, only names acceptable on import,
/// optionally restricted to the given file stems (item class names).
pub fn read_dir_files(dir: &Path, prefix: &str, only: Option<&HashSet<String>>) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(only) = only {
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if !only.contains(&stem) {
                continue;
            }
        }
        if safe_entry(&format!("{prefix}/{name}")).is_none() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            out.push((format!("{prefix}/{name}"), bytes));
        }
    }
    out
}

/// Zip `entries` behind a manifest (signed with `keys` when given). `manifest.files` is
/// filled in here. Entry order and timestamps are fixed, so identical content gives an
/// identical zip.
pub fn build_bundle(entries: &[(String, Vec<u8>)], manifest: &mut Manifest, keys: Option<&sign::Keys>) -> std::io::Result<Vec<u8>> {
    manifest.files = entries.iter().map(|(p, b)| ManifestFile { path: p.clone(), sha256: sign::sha256_hex(b), size: b.len() as u64 }).collect();
    if let Some(k) = keys {
        manifest.author.public_key = Some(k.public_key());
    }
    let manifest_bytes = serde_json::to_vec_pretty(manifest)?;
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zw.start_file(MANIFEST_FILE, opts)?;
    zw.write_all(&manifest_bytes)?;
    if let Some(k) = keys {
        zw.start_file(SIGNATURE_FILE, opts)?;
        zw.write_all(k.sign(&manifest_bytes).as_bytes())?;
    }
    for (path, bytes) in entries {
        zw.start_file(path, opts)?;
        zw.write_all(bytes)?;
    }
    Ok(zw.finish()?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn entry_validation() {
        assert_eq!(safe_entry("img/ship.png"), Some((EntryKind::Img, "ship.png".into())));
        assert_eq!(safe_entry("render/Armor_Helmet.png"), Some((EntryKind::Render, "Armor_Helmet.png".into())));
        assert_eq!(safe_entry("custom/x.webp"), Some((EntryKind::Custom, "x.webp".into())));
        assert!(safe_entry("../x.png").is_none());
        assert!(safe_entry("img/../x.png").is_none());
        assert!(safe_entry("/img/x.png").is_none());
        assert!(safe_entry("img/sub/x.png").is_none());
        assert!(safe_entry("img\\x.png").is_none());
        assert!(safe_entry("other/x.png").is_none());
        assert!(safe_entry("img/x.exe").is_none());
    }

    fn manifest() -> Manifest {
        Manifest {
            format: MANIFEST_FORMAT,
            channel: "LIVE".into(),
            version: "1".into(),
            label: "1".into(),
            created_at: String::new(),
            author: Author { name: "t".into(), handle: "t".into(), public_key: None, mode: "watcher".into() },
            files: Vec::new(),
            content_hash: Some("v1:abc".into()),
            extractor: None,
        }
    }

    #[test]
    fn builds_signed_zip() {
        let keys = sign::Keys::from_seed([1u8; 32]);
        let entries = vec![(DATASET_FILE.to_string(), b"{}".to_vec()), ("img/a.png".to_string(), vec![1, 2, 3])];
        let mut m = manifest();
        let zip_bytes = build_bundle(&entries, &mut m, Some(&keys)).unwrap();
        assert_eq!(m.files.len(), 2);
        assert_eq!(m.files[1].sha256, sign::sha256_hex(&[1, 2, 3]));
        assert_eq!(m.author.public_key.as_deref(), Some(keys.public_key().as_str()));
        let mut za = zip::ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
        let mut manifest = Vec::new();
        za.by_name(MANIFEST_FILE).unwrap().read_to_end(&mut manifest).unwrap();
        let mut sig = String::new();
        za.by_name(SIGNATURE_FILE).unwrap().read_to_string(&mut sig).unwrap();
        assert!(sign::verify(&keys.public_key(), &manifest, &sig).is_ok());
        assert_eq!(file_name("live", "4.10.2"), "wikelo-LIVE-4.10.2.zip");
    }

    #[test]
    fn unsigned_zip_is_reproducible() {
        let entries = vec![(DATASET_FILE.to_string(), b"{}".to_vec()), ("img/a.png".to_string(), vec![1, 2, 3])];
        let a = build_bundle(&entries, &mut manifest(), None).unwrap();
        let b = build_bundle(&entries, &mut manifest(), None).unwrap();
        assert_eq!(a, b, "same content, same bytes");
        let mut za = zip::ZipArchive::new(Cursor::new(a)).unwrap();
        assert!(za.by_name(SIGNATURE_FILE).is_err(), "no signature entry");
        let mut manifest_bytes = Vec::new();
        za.by_name(MANIFEST_FILE).unwrap().read_to_end(&mut manifest_bytes).unwrap();
        let m: Manifest = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(m.format, 2);
        assert_eq!(m.content_hash.as_deref(), Some("v1:abc"));
        assert!(m.author.public_key.is_none());
        // Format 1 manifests (signed, no contentHash) still parse.
        let v1 = r#"{"format":1,"channel":"LIVE","version":"1","label":"1","createdAt":"","author":{"name":"t","handle":"t","publicKey":"k","mode":"local"},"files":[]}"#;
        let m1: Manifest = serde_json::from_str(v1).unwrap();
        assert_eq!(m1.author.public_key.as_deref(), Some("k"));
        assert!(m1.content_hash.is_none());
    }
}
