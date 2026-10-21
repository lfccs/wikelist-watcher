//! On-demand 3D export (GLB) of an entity class — ship, armor piece, weapon, carryable,
//! resource thumbnail — through `starbreaker-3d`, so the UI can show a real model and
//! render its own thumbnail the way the game does for its inventory icons.

use std::path::Path;
use std::time::Instant;

use starbreaker_datacore::Database;
use starbreaker_datacore::loadout::{EntityIndex, resolve_loadout_indexed};
use starbreaker_p4k::MappedP4k;
use starbreaker_3d::{ExportOptions, MaterialMode};

#[derive(Debug, thiserror::Error)]
pub enum GlbError {
    #[error("cannot open Data.p4k: {0}")]
    P4k(String),
    #[error("cannot read Game2.dcb: {0}")]
    Dcb(String),
    #[error("entity class {0} not found")]
    NotFound(String),
    #[error("export failed for {0}: {1}")]
    Export(String, String),
}

/// How much of the model to export. Ships are huge; thumbnails only need the hull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDetail {
    /// Full item with attachments, textures at 1/4 resolution (weapons, carryables).
    Item,
    /// Armor as worn: the record's root geometry is the crate the piece ships in, the piece
    /// itself is the sub-geometry tagged by body ("male" / "female"), a skinned mesh in bind
    /// pose, upright and facing forward, which is how the game's inventory shows it.
    Worn,
    /// Exterior only, lower LOD, textures at 1/16 resolution (vehicles).
    VehicleExterior,
}

impl ModelDetail {
    pub fn for_item_kind(kind: &str) -> Self {
        match kind {
            "vehicle" => ModelDetail::VehicleExterior,
            "armor" => ModelDetail::Worn,
            _ => ModelDetail::Item,
        }
    }

    fn options(self) -> ExportOptions {
        let mut o = ExportOptions { material_mode: MaterialMode::Textures, ..ExportOptions::default() };
        match self {
            ModelDetail::Item => {
                o.lod_level = 0;
                o.texture_mip = 1;
            }
            ModelDetail::Worn => {
                o.lod_level = 0;
                o.texture_mip = 1;
                o.geometry_tag = Some("male".into());
            }
            ModelDetail::VehicleExterior => {
                o.include_interior = false;
                o.include_lights = false;
                o.lod_level = 2;
                o.texture_mip = 3;
            }
        }
        o
    }
}

/// Keeps the archive and the DataForge bytes open so several exports can share
/// one parse of `Game2.dcb`.
pub struct ModelExporter {
    p4k: MappedP4k,
    dcb: Vec<u8>,
}

impl ModelExporter {
    pub fn open(p4k_path: &Path) -> Result<Self, GlbError> {
        let p4k = MappedP4k::open(p4k_path).map_err(|e| GlbError::P4k(e.to_string()))?;
        let dcb = p4k
            .read_file("Data\\Game2.dcb")
            .or_else(|_| p4k.read_file("Data\\Game.dcb"))
            .map_err(|e| GlbError::Dcb(e.to_string()))?;
        Ok(Self { p4k, dcb })
    }

    /// Export one entity class as GLB bytes.
    /// One file of the archive as the server needs it (localization tables, for one).
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, GlbError> {
        self.p4k.read_file(path).map_err(|e| GlbError::P4k(format!("{path}: {e}")))
    }

    pub fn export(&self, class_name: &str, detail: ModelDetail) -> Result<Vec<u8>, GlbError> {
        let started = Instant::now();
        let db = Database::from_bytes(&self.dcb).map_err(|e| GlbError::Dcb(e.to_string()))?;
        let wanted = format!("EntityClassDefinition.{class_name}");
        let record = db
            .records_by_type_name("EntityClassDefinition")
            .find(|r| db.resolve_string2(r.name_offset).eq_ignore_ascii_case(&wanted))
            .copied()
            .ok_or_else(|| GlbError::NotFound(class_name.to_string()))?;
        let idx = EntityIndex::new(&db);
        let tree = resolve_loadout_indexed(&idx, &record);
        let assemble = |opts: &ExportOptions| starbreaker_3d::assemble_glb_with_loadout_with_progress(&db, &self.p4k, &record, &tree, opts, None, None, None, None);
        let result = match assemble(&detail.options()) {
            // Not every armor record carries a body-tagged variant (older pieces, flair): the crate
            // or prop the record names is still better than nothing.
            Err(e) if detail == ModelDetail::Worn && e.to_string().contains("geometry variant") => {
                tracing::info!("GLB {class_name}: sem variante vestida ({e}); exportando a geometria do registro");
                assemble(&ModelDetail::Item.options())
            }
            other => other,
        }
        .map_err(|e| GlbError::Export(class_name.to_string(), e.to_string()))?;
        tracing::info!(
            "GLB {class_name}: {} bytes em {:.1}s (geometry {})",
            result.glb.len(),
            started.elapsed().as_secs_f64(),
            result.geometry_path
        );
        if result.glb.is_empty() {
            return Err(GlbError::Export(class_name.to_string(), "empty export".into()));
        }
        Ok(result.glb)
    }
}
