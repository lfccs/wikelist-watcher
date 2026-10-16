//! Build a bundle for one install: used by `wikelo-watcher bundle`, `wikelo-watcher hash`
//! and by the background watcher when it sends a new game version to the server.

use wikelo_install::InstallInfo;
use wikelo_watcher::bundle::{Author, DATASET_FILE, MANIFEST_FORMAT, Manifest, build_bundle, file_name, safe_entry};
use wikelo_watcher::{ExtractOutput, ExtractProgress, extract};

pub struct Built {
    pub bytes: Vec<u8>,
    pub file_name: String,
    pub label: String,
    pub contracts: usize,
    pub items: usize,
    pub images: usize,
    pub skipped: usize,
    pub warnings: Vec<String>,
    /// Reproducible identity of the dataset (`wikelo_core::canonical_hash`).
    pub content_hash: String,
    pub extractor: Option<wikelo_core::ExtractorInfo>,
}

/// Name recorded as the author of bundles made on this PC.
pub fn author_name(name: Option<String>) -> String {
    name.or_else(|| std::env::var("USERNAME").ok())
        .or_else(|| std::env::var("USER").ok())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "jogador".into())
}

/// Extract the install's contracts and images and zip them behind a manifest. No
/// signature: the server validates the content itself and recomputes the hash.
/// `name`/`handle` are recorded as the author (the account on the server, when signed in).
pub fn make_bundle(install: &InstallInfo, name: &str, handle: &str, on_progress: &dyn Fn(ExtractProgress)) -> anyhow::Result<Built> {
    let ExtractOutput { dataset, images } = extract(install, on_progress)?;
    let label = wikelo_core::version_label(&dataset.build.branch, &dataset.build.version);
    let content_hash = wikelo_core::canonical_hash(&dataset);
    let mut entries: Vec<(String, Vec<u8>)> = vec![(DATASET_FILE.to_string(), serde_json::to_vec(&dataset)?)];
    let mut skipped = 0;
    for img in images {
        let path = img.file.replace('\\', "/");
        if safe_entry(&path).is_some() {
            entries.push((path, img.bytes));
        } else {
            skipped += 1;
        }
    }
    let mut manifest = Manifest {
        format: MANIFEST_FORMAT,
        channel: dataset.build.channel.to_ascii_uppercase(),
        version: dataset.build.version.clone(),
        label: label.clone(),
        created_at: time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        author: Author { name: name.to_string(), handle: handle.to_string(), public_key: None, mode: "watcher".into() },
        files: Vec::new(),
        content_hash: Some(content_hash.clone()),
        extractor: dataset.extractor.clone(),
    };
    let bytes = build_bundle(&entries, &mut manifest, None)?;
    Ok(Built {
        bytes,
        file_name: file_name(&dataset.build.channel, &label),
        label,
        contracts: dataset.contracts.len(),
        items: dataset.items.len(),
        images: entries.len() - 1,
        skipped,
        warnings: dataset.warnings,
        content_hash,
        extractor: dataset.extractor,
    })
}
