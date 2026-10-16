//! Data.p4k -> Game2.dcb -> `ContractGenerator.TheCollector` -> normalized dataset.

pub mod glb;
pub mod bundle;
pub mod images;
pub mod sign;

/// Where this machine keeps its key, watcher session and (for the app) its database:
/// `WIKELO_DATA_DIR` or `%LOCALAPPDATA%\wikelo-sc\data`, the same place the local app uses.
pub fn data_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("WIKELO_DATA_DIR")
        && !d.trim().is_empty()
    {
        return std::path::PathBuf::from(d);
    }
    directories::ProjectDirs::from("", "", "wikelo-sc")
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("wikelo-sc-data"))
}
pub mod loc;
pub mod normalize;
pub mod resolver;
pub mod tree;

use std::path::Path;
use std::time::Instant;

use starbreaker_common::Progress;
use starbreaker_datacore::Database;
use starbreaker_p4k::MappedP4k;
use wikelo_core::{BuildInfo, WikeloDataset};
use wikelo_install::InstallInfo;

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("Data.p4k not found at {0}")]
    P4kMissing(std::path::PathBuf),
    #[error("cannot open Data.p4k: {0}")]
    P4k(String),
    #[error("cannot read {path} from Data.p4k: {detail}")]
    P4kEntry { path: String, detail: String },
    #[error("DataCore parse error: {0}")]
    Dcb(String),
    #[error("DataCore version {0} is not supported by this build (expected 6 or 8)")]
    DcbVersion(u32),
    #[error(transparent)]
    Normalize(#[from] normalize::NormalizeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractProgress {
    pub stage: &'static str,
    /// 0..=100
    pub pct: u8,
    pub message: String,
    pub elapsed_sec: f64,
}

/// Raw game files needed for extraction.
pub struct GameData {
    pub p4k: MappedP4k,
    pub dcb: Vec<u8>,
    pub ini: Vec<u8>,
}

impl GameData {
    pub fn open(p4k_path: &Path, progress: Option<&Progress>) -> Result<Self, ExtractError> {
        if !p4k_path.is_file() {
            return Err(ExtractError::P4kMissing(p4k_path.to_path_buf()));
        }
        let p4k = MappedP4k::open_with_progress(p4k_path, progress).map_err(|e| ExtractError::P4k(e.to_string()))?;
        let dcb = p4k
            .read_file("Data\\Game2.dcb")
            .or_else(|_| p4k.read_file("Data\\Game.dcb"))
            .map_err(|e| ExtractError::P4kEntry { path: "Data\\Game2.dcb".into(), detail: e.to_string() })?;
        let ini = p4k
            .read_file("Data\\Localization\\english\\global.ini")
            .map_err(|e| ExtractError::P4kEntry { path: "Data\\Localization\\english\\global.ini".into(), detail: e.to_string() })?;
        Ok(Self { p4k, dcb, ini })
    }

    /// DataForge header version (bytes 4..8, little endian).
    pub fn dcb_version(&self) -> Option<u32> {
        self.dcb.get(4..8).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Everything one extraction produces: the dataset plus the item images that
/// go into `<dataset dir>/img/`.
pub struct ExtractOutput {
    pub dataset: WikeloDataset,
    pub images: Vec<images::ExtractedImage>,
}

impl ExtractOutput {
    /// Write the images under `dir` (creating `dir/img`). Returns how many were written.
    pub fn write_images(&self, dir: &Path) -> std::io::Result<usize> {
        let mut n = 0;
        for img in &self.images {
            let path = dir.join(&img.file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, &img.bytes)?;
            n += 1;
        }
        Ok(n)
    }
}

/// Run the full extraction for one install.
pub fn extract(install: &InstallInfo, on_progress: &dyn Fn(ExtractProgress)) -> Result<ExtractOutput, ExtractError> {
    let started = Instant::now();
    let report = |stage: &'static str, pct: u8, message: String| {
        on_progress(ExtractProgress { stage, pct, message, elapsed_sec: started.elapsed().as_secs_f64() });
    };

    report("p4k", 0, format!("Abrindo {}", install.p4k_path.display()));
    let data = GameData::open(&install.p4k_path, None)?;
    report("p4k", 40, format!("{} entradas no Data.p4k", data.p4k.len()));

    report("dcb", 45, format!("Game2.dcb lido ({} bytes)", data.dcb.len()));
    let dcb_version = data.dcb_version();
    let db = Database::from_bytes(&data.dcb).map_err(|e| match e {
        starbreaker_datacore::ParseError::UnsupportedVersion(v) => ExtractError::DcbVersion(v),
        other => ExtractError::Dcb(other.to_string()),
    })?;
    report(
        "dcb",
        60,
        format!("DataCore v{}: {} registros", dcb_version.map(|v| v.to_string()).unwrap_or_else(|| "?".into()), db.records().len()),
    );

    report("loc", 65, "Lendo global.ini".into());
    let loc = loc::Localization::parse(&data.ini);
    report("loc", 70, format!("{} chaves de localização", loc.len()));

    report("normalize", 75, "Normalizando contratos do Wikelo".into());
    let mut resolver = resolver::RefResolver::new(&db);
    let mut build: BuildInfo = install.build_info();
    build.dcb_version = dcb_version;
    let generated_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let mut dataset = normalize::Normalizer::new(&mut resolver, &loc).run(build, generated_at)?;
    // Provenance: which build of this code read which exact game files. Two players on the
    // same build must report the same hashes, and the server shows them next to the dataset.
    dataset.extractor = Some(wikelo_core::ExtractorInfo {
        name: "wikelo-watcher".into(),
        version: wikelo_core::PKG_VERSION.into(),
        commit: wikelo_core::GIT_COMMIT.into(),
        dcb_sha256: wikelo_core::sha256_hex(&data.dcb),
        ini_sha256: wikelo_core::sha256_hex(&data.ini),
    });
    report("normalize", 85, format!("{} contratos, {} itens", dataset.contracts.len(), dataset.items.len()));

    report("images", 86, "Procurando imagens dos itens no Data.p4k".into());
    let mut on_img = |n: usize, total: usize| {
        if let Some(step) = (n * 13).checked_div(total) {
            report("images", 86 + step as u8, format!("Convertendo imagens ({n}/{total})"));
        }
    };
    let images = images::attach_images(&data.p4k, &mut dataset.items, &mut dataset.warnings, &mut on_img);
    let with_image = dataset.items.iter().filter(|i| i.image.is_some()).count();
    report(
        "done",
        100,
        format!(
            "{} contratos, {} itens ({} com imagem), {} avisos",
            dataset.contracts.len(),
            dataset.items.len(),
            with_image,
            dataset.warnings.len()
        ),
    );
    Ok(ExtractOutput { dataset, images })
}
