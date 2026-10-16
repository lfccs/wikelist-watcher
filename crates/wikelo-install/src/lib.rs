//! Discovery of Star Citizen installations (LIVE / PTU / EPTU / HOTFIX /
//! TECH-PREVIEW) and parsing of each channel's `build_manifest.id`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wikelo_core::BuildInfo;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("build_manifest.id not found in {0}")]
    ManifestMissing(PathBuf),
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Contents of `<channel>\build_manifest.id` (the `Data` object).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub struct BuildManifest {
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub build_date_stamp: String,
    #[serde(default)]
    pub build_id: String,
    #[serde(default)]
    pub build_time_stamp: String,
    #[serde(default)]
    pub config: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default, rename = "RequestedP4ChangeNum")]
    pub requested_p4_change_num: String,
    #[serde(default)]
    pub shelved_change: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(rename = "Data")]
    data: BuildManifest,
}

/// One installed channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstallInfo {
    /// Uppercase channel name derived from the folder (LIVE, PTU, ...).
    pub channel: String,
    pub root: PathBuf,
    pub p4k_path: PathBuf,
    pub game_log_path: PathBuf,
    pub logbackups_dir: PathBuf,
    pub manifest: BuildManifest,
    pub p4k_size: Option<u64>,
}

impl InstallInfo {
    /// Stable cache key: `LIVE-4.10.197.2878-12913470`.
    pub fn cache_key(&self) -> String {
        format!(
            "{}-{}-{}",
            self.channel,
            sanitize(&self.manifest.version),
            sanitize(&self.manifest.requested_p4_change_num)
        )
    }

    pub fn build_info(&self) -> BuildInfo {
        BuildInfo {
            channel: self.channel.clone(),
            branch: self.manifest.branch.clone(),
            version: self.manifest.version.clone(),
            p4_change: self.manifest.requested_p4_change_num.clone(),
            build_id: self.manifest.build_id.clone(),
            tag: self.manifest.tag.clone(),
            dcb_version: None,
            p4k_size: self.p4k_size,
        }
    }

    pub fn has_p4k(&self) -> bool {
        self.p4k_path.is_file()
    }
}

fn sanitize(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    if t.is_empty() { "unknown".into() } else { t }
}

pub fn read_build_manifest(channel_dir: &Path) -> Result<BuildManifest, InstallError> {
    let path = channel_dir.join("build_manifest.id");
    if !path.is_file() {
        return Err(InstallError::ManifestMissing(channel_dir.to_path_buf()));
    }
    let text = fs::read_to_string(&path).map_err(|source| InstallError::Io { path: path.clone(), source })?;
    let text = text.trim_start_matches('\u{feff}');
    let file: ManifestFile =
        serde_json::from_str(text).map_err(|source| InstallError::Json { path: path.clone(), source })?;
    Ok(file.data)
}

/// Build an `InstallInfo` from a channel directory (one containing `build_manifest.id`).
pub fn install_from_channel_dir(channel_dir: &Path) -> Result<InstallInfo, InstallError> {
    let manifest = read_build_manifest(channel_dir)?;
    let channel = channel_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "UNKNOWN".into());
    let p4k_path = channel_dir.join("Data.p4k");
    let p4k_size = fs::metadata(&p4k_path).ok().map(|m| m.len());
    Ok(InstallInfo {
        channel,
        root: channel_dir.to_path_buf(),
        p4k_path,
        game_log_path: channel_dir.join("Game.log"),
        logbackups_dir: channel_dir.join("logbackups"),
        manifest,
        p4k_size,
    })
}

/// Relative layouts (under a drive root) where the launcher is known to put the library.
const KNOWN_LAYOUTS: &[&str] = &[
    "Program Files\\Roberts Space Industries\\StarCitizen",
    "Program Files (x86)\\Roberts Space Industries\\StarCitizen",
    "Roberts Space Industries\\StarCitizen",
    "Games\\Roberts Space Industries\\StarCitizen",
    "games\\StarCitizen",
    "Games\\StarCitizen",
    "StarCitizen\\StarCitizen",
    "StarCitizen",
    "Star Citizen",
];

/// Candidate library roots: known layouts on every available drive letter,
/// plus `SC_INSTALL_ROOTS` (semicolon-separated) and any `extra` roots.
pub fn candidate_roots(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(env) = std::env::var("SC_INSTALL_ROOTS") {
        for part in env.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            roots.push(PathBuf::from(part));
        }
    }
    roots.extend(extra.iter().cloned());
    for letter in b'A'..=b'Z' {
        let drive = PathBuf::from(format!("{}:\\", letter as char));
        if !drive.is_dir() {
            continue;
        }
        for layout in KNOWN_LAYOUTS {
            roots.push(drive.join(layout));
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Discover every installed channel. A root is either a library folder whose
/// children are channel folders, or a channel folder itself.
pub fn discover_installs(extra_roots: &[PathBuf]) -> Vec<InstallInfo> {
    let mut found: Vec<InstallInfo> = Vec::new();
    for root in candidate_roots(extra_roots) {
        if !root.is_dir() {
            continue;
        }
        if root.join("build_manifest.id").is_file() {
            if let Ok(info) = install_from_channel_dir(&root) {
                push_unique(&mut found, info);
            }
            continue;
        }
        let Ok(entries) = fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("build_manifest.id").is_file() {
                match install_from_channel_dir(&path) {
                    Ok(info) => push_unique(&mut found, info),
                    Err(e) => tracing::warn!("skipping {}: {e}", path.display()),
                }
            }
        }
    }
    found.sort_by(|a, b| {
        channel_rank(&a.channel)
            .cmp(&channel_rank(&b.channel))
            .then_with(|| a.channel.cmp(&b.channel))
    });
    found
}

fn push_unique(list: &mut Vec<InstallInfo>, info: InstallInfo) {
    // Windows paths are case-insensitive; the same install can be reached as
    // `G:\Games\...` and `G:\games\...`.
    let key = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\");
    if !list.iter().any(|i| key(&i.root) == key(&info.root)) {
        list.push(info);
    }
}

fn channel_rank(channel: &str) -> u8 {
    match channel {
        "LIVE" => 0,
        "HOTFIX" => 1,
        "PTU" => 2,
        "EPTU" => 3,
        "TECH-PREVIEW" => 4,
        _ => 9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE_MANIFEST: &str = r#"{
    "Data": {
        "Branch": "sc-alpha-4.10.2",
        "BuildDateStamp": "Mon Oct 12 2026",
        "BuildId": "2cae0dfc-a941-4dcc-b82a-543b08b52d97",
        "BuildTimeStamp": "02:20:01 PM CST",
        "Config": "shipping",
        "Platform": "pc",
        "RequestedP4ChangeNum": "12913470",
        "Shelved_Change": "",
        "Tag": "public",
        "Version": "4.10.197.2878"
    }
}"#;

    const PTU_MANIFEST: &str = r#"{"Data":{"Branch":"sc-alpha-4.9.0","BuildDateStamp":"Thu Jul 02 2026","BuildId":"9f740175-d112-3f94-8df4-1e7ca2a9f1b6","BuildTimeStamp":"04:54:33 PM CST","Config":"shipping","Platform":"pc","RequestedP4ChangeNum":"12148226","Shelved_Change":"","Tag":"public","Version":"4.9.185.24066"}}"#;

    #[test]
    fn parses_live_manifest_and_builds_cache_key() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("LIVE");
        fs::create_dir(&live).unwrap();
        fs::write(live.join("build_manifest.id"), LIVE_MANIFEST).unwrap();
        let info = install_from_channel_dir(&live).unwrap();
        assert_eq!(info.channel, "LIVE");
        assert_eq!(info.manifest.version, "4.10.197.2878");
        assert_eq!(info.manifest.requested_p4_change_num, "12913470");
        assert_eq!(info.manifest.branch, "sc-alpha-4.10.2");
        assert_eq!(info.cache_key(), "LIVE-4.10.197.2878-12913470");
        assert_eq!(info.game_log_path, live.join("Game.log"));
        assert!(!info.has_p4k());
    }

    #[test]
    fn discovers_channels_under_a_library_root() {
        let dir = tempfile::tempdir().unwrap();
        for (ch, m) in [("LIVE", LIVE_MANIFEST), ("ptu", PTU_MANIFEST)] {
            let d = dir.path().join(ch);
            fs::create_dir(&d).unwrap();
            fs::write(d.join("build_manifest.id"), m).unwrap();
        }
        fs::create_dir(dir.path().join("not-a-channel")).unwrap();
        let found = discover_installs(&[dir.path().to_path_buf()]);
        let ours: Vec<&InstallInfo> = found.iter().filter(|i| i.root.starts_with(dir.path())).collect();
        assert_eq!(ours.len(), 2);
        assert_eq!(ours[0].channel, "LIVE");
        assert_eq!(ours[1].channel, "PTU");
        assert_eq!(ours[1].manifest.version, "4.9.185.24066");
    }

    #[test]
    fn missing_manifest_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(read_build_manifest(dir.path()), Err(InstallError::ManifestMissing(_))));
    }
}
