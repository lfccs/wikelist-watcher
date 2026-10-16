//! Static item imagery taken from `Data.p4k` UI textures.
//!
//! Star Citizen renders most inventory thumbnails at runtime from the 3D
//! models, so there is no "one icon per item" table. What the archive does
//! ship are a few static UI texture sets that happen to cover a good part of
//! what Wikelo hands out:
//!
//! * `Data\UI\Textures\EA\VehicleIcons\VehicleIcon_<MFR>_<Model>.dds` — 1024²
//!   vehicle icons used by Arena Commander.
//! * `Data\UI\Frontend\assets\TIF\Ships\<Model>.dds` — older rendered ship
//!   portraits from the frontend ship selector.
//! * `Data\UI\Textures\EA\LoadoutIcons\<Maker>_<Model>_<Type>_64.dds` — 64²
//!   FPS weapon icons.
//! * `Data\UI\Textures\Vector\General\Items\Inv_Icon_<Name>.svg` — inventory
//!   icons for commodities and some carryables.
//!
//! The index matches dataset items to those files by name tokens, decodes the
//! DDS textures (merging split mips when present) and returns PNG/SVG bytes
//! that the caller writes next to `wikelo-data.json`.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;

use starbreaker_dds::{DdsFile, ReadSibling};
use starbreaker_p4k::MappedP4k;
use wikelo_core::Item;

/// Sub-directory (relative to the dataset directory) that holds the images.
pub const IMG_DIR: &str = "img";
/// Longest side of an exported PNG.
pub const MAX_DIM: u32 = 512;

/// Where an item's image came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageSourceKind {
    VehicleIcon,
    FrontendShip,
    LoadoutIcon,
    InvIcon,
    Generic,
}

impl ImageSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageSourceKind::VehicleIcon => "vehicle_icon",
            ImageSourceKind::FrontendShip => "frontend_ship",
            ImageSourceKind::LoadoutIcon => "loadout_icon",
            ImageSourceKind::InvIcon => "inv_icon",
            ImageSourceKind::Generic => "generic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSource {
    /// Full entry path inside the p4k (backslashes).
    pub entry: String,
    pub kind: ImageSourceKind,
}

impl ImageSource {
    /// File name used on disk / in the dataset (`img/<stem>.png|svg`).
    pub fn file_name(&self) -> String {
        let stem = self.entry.rsplit(['\\', '/']).next().unwrap_or(&self.entry);
        let (base, ext) = match stem.rsplit_once('.') {
            Some((b, e)) => (b, e.to_ascii_lowercase()),
            None => (stem, String::new()),
        };
        let ext = if ext == "svg" { "svg" } else { "png" };
        let safe: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        format!("{IMG_DIR}/{safe}.{ext}")
    }
}

/// A decoded image ready to be written.
#[derive(Debug, Clone)]
pub struct ExtractedImage {
    /// Relative path (`img/...`).
    pub file: String,
    pub bytes: Vec<u8>,
}

/// Generic category icons exported for the UI fallback (`img/generic-<kind>.svg`).
const GENERIC_ICONS: &[(&str, &str)] = &[
    ("armor", r"data\ui\textures\vector\general\items\inv_filter_icons_armor.svg"),
    ("weapon", r"data\ui\textures\vector\general\commonicons\icon_common_weapon_gun.svg"),
    ("helmet", r"data\ui\textures\vector\pit\looting\icon_common_helmet.svg"),
    ("default", r"data\ui\textures\vector\general\items\inv_icon_default.svg"),
    ("favour", r"data\ui\textures\vector\general\items\inv_icon_default.svg"),
];

/// Vehicle class-name tokens that only describe a paint / variant and never a model.
const VEHICLE_VARIANT_TOKENS: &[&str] = &[
    "collector", "military", "milt", "stealth", "mod", "indust", "industrial", "competition", "medic", "civ", "civilian", "gs",
    "ikti", "unique", "special", "wikelo",
];

struct Pool {
    /// (normalized stem, entry path)
    entries: Vec<(String, String)>,
}

impl Pool {
    fn build(p4k: &MappedP4k, dir_token: &str, ext: &str) -> Self {
        let mut idx = p4k.search(dir_token);
        idx.sort_unstable();
        let entries = p4k.entries();
        let mut out = Vec::new();
        for i in idx {
            let name = &entries[i as usize].name;
            let lower = name.to_ascii_lowercase();
            if !lower.contains(dir_token) || !lower.ends_with(ext) {
                continue;
            }
            let stem = lower.rsplit('\\').next().unwrap_or(&lower);
            let stem = &stem[..stem.len() - ext.len()];
            if stem.starts_with('_') {
                continue; // `_Missing.dds`
            }
            out.push((norm(stem), name.clone()));
        }
        // Ties ("shortest stem wins", "first match") must not depend on the order of the
        // p4k directory, which can differ between a fresh and a patched install.
        out.sort();
        Self { entries: out }
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Lowercase, alphanumerics only.
pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// Index of candidate images found in the archive.
pub struct ImageIndex {
    vehicle_icons: Pool,
    frontend_ships: Pool,
    loadout_icons: Pool,
    inv_icons: Pool,
}

impl ImageIndex {
    pub fn new(p4k: &MappedP4k) -> Self {
        Self {
            vehicle_icons: Pool::build(p4k, r"\ui\textures\ea\vehicleicons\", ".dds"),
            frontend_ships: Pool::build(p4k, r"\ui\frontend\assets\tif\ships\", ".dds"),
            loadout_icons: Pool::build(p4k, r"\ui\textures\ea\loadouticons\", ".dds"),
            inv_icons: Pool::build(p4k, r"\ui\textures\vector\general\items\inv_icon_", ".svg"),
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "{} vehicle icons, {} frontend ships, {} loadout icons, {} inventory icons",
            self.vehicle_icons.entries.len(),
            self.frontend_ships.entries.len(),
            self.loadout_icons.entries.len(),
            self.inv_icons.entries.len()
        )
    }

    /// Best static image for an item, if any.
    pub fn resolve(&self, item: &Item) -> Option<ImageSource> {
        match item.kind.as_str() {
            "vehicle" => self.resolve_vehicle(&item.class_name),
            "weapon" => self.resolve_weapon(item),
            _ => self.resolve_inv_icon(&item.name),
        }
    }

    fn resolve_vehicle(&self, class_name: &str) -> Option<ImageSource> {
        let tokens: Vec<String> = class_name
            .split('_')
            .map(|t| t.to_ascii_lowercase())
            .take_while(|t| t != "collector")
            .collect();
        if tokens.len() < 2 {
            return None;
        }
        // Drop trailing variant tokens (RSI_Constellation_Taurus_GS_Military -> RSI Constellation Taurus).
        let mut model: Vec<&str> = tokens[1..].iter().map(String::as_str).collect();
        while model.len() > 1 && VEHICLE_VARIANT_TOKENS.contains(&model.last().copied().unwrap_or("")) {
            model.pop();
        }
        let mfr = tokens[0].as_str();

        // 1) Frontend ship portraits: real renders, but only for older ships.
        if !self.frontend_ships.is_empty() {
            for k in (1..=model.len()).rev() {
                let joined: String = model[..k].concat();
                if joined.len() < 4 {
                    continue;
                }
                let mut best: Option<(usize, &str)> = None;
                for (stem, entry) in &self.frontend_ships.entries {
                    let ok = *stem == joined || stem.starts_with(&joined) || stem.ends_with(&joined);
                    if ok && best.is_none_or(|(len, _)| stem.len() < len) {
                        best = Some((stem.len(), entry.as_str()));
                    }
                }
                if let Some((_, entry)) = best {
                    return Some(ImageSource { entry: entry.to_string(), kind: ImageSourceKind::FrontendShip });
                }
            }
        }
        // 2) Arena Commander vehicle icons (white silhouettes, more ships).
        if !self.vehicle_icons.is_empty() {
            for k in (1..=model.len()).rev() {
                let joined: String = model[..k].concat();
                let with_mfr = format!("vehicleicon{mfr}{joined}");
                let without = format!("vehicleicon{joined}");
                let hit = best_prefix(&self.vehicle_icons, &with_mfr).or_else(|| best_prefix(&self.vehicle_icons, &without));
                if let Some(entry) = hit {
                    return Some(ImageSource { entry, kind: ImageSourceKind::VehicleIcon });
                }
            }
        }
        None
    }

    fn resolve_weapon(&self, item: &Item) -> Option<ImageSource> {
        if self.loadout_icons.is_empty() {
            return None;
        }
        // "F55 \"Shadowflux\" LMG" -> f55 ; "Ravager-212 \"Red Alert\" Twin Shotgun" -> ravager212
        let first = item.name.split_whitespace().next()?;
        let token = norm(first);
        if token.len() < 3 || token.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut best: Option<(usize, &str)> = None;
        for (stem, entry) in &self.loadout_icons.entries {
            if stem.contains(&token) && best.is_none_or(|(len, _)| stem.len() < len) {
                best = Some((stem.len(), entry.as_str()));
            }
        }
        best.map(|(_, entry)| ImageSource { entry: entry.to_string(), kind: ImageSourceKind::LoadoutIcon })
    }

    fn resolve_inv_icon(&self, name: &str) -> Option<ImageSource> {
        if self.inv_icons.is_empty() {
            return None;
        }
        let whole = norm(name);
        let words: Vec<String> = name
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .map(norm)
            .filter(|w| w.len() >= 4 && !w.chars().all(|c| c.is_ascii_digit()))
            .collect();
        let mut best: Option<&str> = None;
        for (stem, entry) in &self.inv_icons.entries {
            let Some(suffix) = stem.strip_prefix("invicon") else { continue };
            if suffix == whole {
                return Some(ImageSource { entry: entry.clone(), kind: ImageSourceKind::InvIcon });
            }
            if best.is_none() && words.iter().any(|w| w == suffix) {
                best = Some(entry.as_str());
            }
        }
        best.map(|entry| ImageSource { entry: entry.to_string(), kind: ImageSourceKind::InvIcon })
    }

    /// Generic category icons present in this archive.
    pub fn generic_icons(&self, p4k: &MappedP4k) -> Vec<(String, ImageSource)> {
        GENERIC_ICONS
            .iter()
            .filter_map(|(kind, path)| {
                p4k.entry_case_insensitive(path).map(|e| {
                    (format!("{IMG_DIR}/generic-{kind}.svg"), ImageSource { entry: e.name.clone(), kind: ImageSourceKind::Generic })
                })
            })
            .collect()
    }
}

/// Entry whose normalized stem starts with `prefix`; shortest stem wins.
fn best_prefix(pool: &Pool, prefix: &str) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for (stem, entry) in &pool.entries {
        if stem.starts_with(prefix) && best.is_none_or(|(len, _)| stem.len() < len) {
            best = Some((stem.len(), entry.as_str()));
        }
    }
    best.map(|(_, e)| e.to_string())
}

struct P4kSiblings<'a> {
    p4k: &'a MappedP4k,
    base: &'a str,
}

impl ReadSibling for P4kSiblings<'_> {
    fn read_sibling(&self, suffix: &str) -> Option<Vec<u8>> {
        let path = format!("{}{}", self.base, suffix);
        let entry = self.p4k.entry(&path).or_else(|| self.p4k.entry_case_insensitive(&path))?;
        self.p4k.read(entry).ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("cannot read {0}: {1}")]
    Read(String, String),
    #[error("cannot decode {0}: {1}")]
    Decode(String, String),
    #[error("cannot encode {0}: {1}")]
    Encode(String, String),
}

/// Read and convert one source into PNG (DDS) or raw SVG bytes.
pub fn render(p4k: &MappedP4k, src: &ImageSource) -> Result<Vec<u8>, ImageError> {
    let read = |path: &str| -> Result<Vec<u8>, ImageError> {
        let entry = p4k
            .entry(path)
            .or_else(|| p4k.entry_case_insensitive(path))
            .ok_or_else(|| ImageError::Read(path.to_string(), "entry not found".into()))?;
        p4k.read(entry).map_err(|e| ImageError::Read(path.to_string(), e.to_string()))
    };
    let bytes = read(&src.entry)?;
    if src.entry.to_ascii_lowercase().ends_with(".svg") {
        return Ok(bytes);
    }
    let dds = DdsFile::from_split(&bytes, &P4kSiblings { p4k, base: &src.entry })
        .map_err(|e| ImageError::Decode(src.entry.clone(), e.to_string()))?;
    let rgba = dds.decode_rgba(0).map_err(|e| ImageError::Decode(src.entry.clone(), e.to_string()))?;
    let (w, h) = dds.dimensions(0);
    let img = image::RgbaImage::from_raw(w, h, rgba)
        .ok_or_else(|| ImageError::Decode(src.entry.clone(), "pixel buffer size mismatch".into()))?;
    let img = if src.kind == ImageSourceKind::FrontendShip { crop_dark_borders(&img) } else { img };
    let (w, h) = img.dimensions();
    let img = if w.max(h) > MAX_DIM {
        let scale = MAX_DIM as f64 / w.max(h) as f64;
        let nw = ((w as f64 * scale).round() as u32).max(1);
        let nh = ((h as f64 * scale).round() as u32).max(1);
        image::imageops::resize(&img, nw, nh, image::imageops::FilterType::CatmullRom)
    } else {
        img
    };
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| ImageError::Encode(src.entry.clone(), e.to_string()))?;
    Ok(out.into_inner())
}

/// Trim uniform transparent / near-black borders (frontend portraits are
/// 512×256 canvases with the ship on the left and empty space for text).
fn crop_dark_borders(img: &image::RgbaImage) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    let dark = |p: &image::Rgba<u8>| p[3] < 8 || (p[0] < 18 && p[1] < 18 && p[2] < 18);
    let col_dark = |x: u32| (0..h).all(|y| dark(img.get_pixel(x, y)));
    let row_dark = |y: u32| (0..w).all(|x| dark(img.get_pixel(x, y)));
    let mut x0 = 0;
    while x0 + 1 < w && col_dark(x0) {
        x0 += 1;
    }
    let mut x1 = w;
    while x1 > x0 + 1 && col_dark(x1 - 1) {
        x1 -= 1;
    }
    let mut y0 = 0;
    while y0 + 1 < h && row_dark(y0) {
        y0 += 1;
    }
    let mut y1 = h;
    while y1 > y0 + 1 && row_dark(y1 - 1) {
        y1 -= 1;
    }
    // Keep a small margin and never crop more than 60% of the width.
    let pad = 6;
    let x0 = x0.saturating_sub(pad);
    let y0 = y0.saturating_sub(pad);
    let x1 = (x1 + pad).min(w);
    let y1 = (y1 + pad).min(h);
    if x1 <= x0 || y1 <= y0 || (x1 - x0) * 10 < w * 4 {
        return img.clone();
    }
    image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image()
}

/// Resolve images for every item (mutating `image` / `image_source`) and render
/// each distinct source once. Failures are reported as warnings, never errors.
pub fn attach_images(
    p4k: &MappedP4k,
    items: &mut [Item],
    warnings: &mut Vec<String>,
    on_progress: &mut dyn FnMut(usize, usize),
) -> Vec<ExtractedImage> {
    let index = ImageIndex::new(p4k);
    tracing::info!("image index: {}", index.summary());

    // file name -> source (dedupe by output file)
    let mut sources: BTreeMap<String, ImageSource> = BTreeMap::new();
    let mut per_item: HashMap<String, (String, ImageSourceKind)> = HashMap::new();
    for item in items.iter() {
        if let Some(src) = index.resolve(item) {
            let file = src.file_name();
            per_item.insert(item.id.clone(), (file.clone(), src.kind));
            sources.entry(file).or_insert(src);
        }
    }
    for (file, src) in index.generic_icons(p4k) {
        sources.entry(file).or_insert(src);
    }

    let total = sources.len();
    let mut out = Vec::with_capacity(total);
    let mut failed: Vec<String> = Vec::new();
    for (n, (file, src)) in sources.iter().enumerate() {
        on_progress(n, total);
        match render(p4k, src) {
            Ok(bytes) => out.push(ExtractedImage { file: file.clone(), bytes }),
            Err(e) => {
                warnings.push(format!("image {file}: {e}"));
                failed.push(file.clone());
            }
        }
    }
    on_progress(total, total);

    for item in items.iter_mut() {
        if let Some((file, kind)) = per_item.get(&item.id)
            && !failed.contains(file)
        {
            item.image = Some(file.clone());
            item.image_source = Some(kind.as_str().to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(stems: &[&str]) -> Pool {
        Pool { entries: stems.iter().map(|s| (norm(s), format!(r"Data\X\{s}.dds"))).collect() }
    }

    fn index(vehicle: &[&str], frontend: &[&str], loadout: &[&str], inv: &[&str]) -> ImageIndex {
        ImageIndex {
            vehicle_icons: pool(vehicle),
            frontend_ships: pool(frontend),
            loadout_icons: pool(loadout),
            inv_icons: pool(inv),
        }
    }

    fn item(kind: &str, class: &str, name: &str) -> Item {
        Item { id: class.into(), class_name: class.into(), name: name.into(), kind: kind.into(), ..Default::default() }
    }

    #[test]
    fn vehicle_icon_matching() {
        let idx = index(
            &["VehicleIcon_RSI_Scorpius", "VehicleIcon_RSI_Ursa_Rover", "VehicleIcon_Idris", "VehicleIcon_ANVL_Hornet", "VehicleIcon_Prowler", "VehicleIcon_RSI_Constellation"],
            &["Idris-P", "Constellation_Taurus", "Sabre"],
            &[],
            &[],
        );
        let e = |c: &str| idx.resolve_vehicle(c).map(|s| s.entry);
        assert!(e("RSI_Scorpius_Stealth").unwrap().contains("RSI_Scorpius"));
        assert!(e("RSI_Ursa_Medivac_Stealth").unwrap().contains("Ursa_Rover"));
        assert!(e("AEGS_Idris_P_Collector_Military").unwrap().contains("Idris-P"));
        assert!(e("ANVL_Hornet_F7_Mk2_Collector_Mod").unwrap().contains("ANVL_Hornet"));
        assert!(e("ESPR_Prowler_Utility_Collector_Indust").unwrap().contains("Prowler"));
        assert!(e("AEGS_Sabre_Firebird_Collector_Milt").unwrap().contains("Sabre"));
        assert!(e("RSI_Constellation_Taurus_GS_Military").unwrap().contains("Constellation_Taurus"));
        assert_eq!(e("KRIG_L21_Wolf_Collector_Military"), None);
    }

    #[test]
    fn weapon_and_inventory_icons() {
        let idx = index(&[], &[], &["Gemini_F55_LMG_64", "Behring_P8_SC_SMG_64", "Kastak_Ravager_212_Twin_Shotgun_64"], &["Inv_Icon_Compboard", "Inv_Icon_Quantainium", "Inv_Icon_Etam"]);
        assert!(idx.resolve(&item("weapon", "gmni_lmg_ballistic_01_collector01", "F55 \"Shadowflux\" LMG")).unwrap().entry.contains("F55"));
        assert!(idx.resolve(&item("weapon", "behr_smg", "P8-SC \"Red Alert\" SMG")).unwrap().entry.contains("P8_SC"));
        assert!(idx.resolve(&item("weapon", "ksar", "Ravager-212 \"Red Alert\" Twin Shotgun")).unwrap().entry.contains("Ravager_212"));
        assert_eq!(idx.resolve(&item("weapon", "volt", "Zenith \"Snow Camo\" Laser Sniper Rifle")), None);
        assert!(idx.resolve(&item("other", "x", "DCHS-05 Orbital Positioning Comp-Board")).unwrap().entry.contains("Compboard"));
        assert!(idx.resolve(&item("commodity", "x", "Quantainium")).unwrap().entry.contains("Quantainium"));
        // "Metamaterial" must not match Inv_Icon_Etam by substring.
        assert_eq!(idx.resolve(&item("other", "x", "Metamaterial Test #146")), None);
    }

    #[test]
    fn file_names() {
        let src = ImageSource { entry: r"Data\UI\Textures\EA\VehicleIcons\VehicleIcon_RSI_Scorpius.dds".into(), kind: ImageSourceKind::VehicleIcon };
        assert_eq!(src.file_name(), "img/VehicleIcon_RSI_Scorpius.png");
        let svg = ImageSource { entry: r"Data\UI\Textures\Vector\General\Items\Inv_Icon_Compboard.svg".into(), kind: ImageSourceKind::InvIcon };
        assert_eq!(svg.file_name(), "img/Inv_Icon_Compboard.svg");
    }
}
