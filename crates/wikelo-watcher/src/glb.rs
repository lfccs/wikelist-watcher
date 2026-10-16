//! On-demand 3D export (GLB) of an entity class — ship, armor piece, weapon —
//! through `starbreaker-3d`, so the UI can show a real model and render its
//! own thumbnail when the game ships no static picture.

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
    /// Full item with attachments, textures at 1/4 resolution (armor, weapons).
    Item,
    /// Exterior only, lower LOD, textures at 1/16 resolution (vehicles).
    VehicleExterior,
}

impl ModelDetail {
    pub fn for_item_kind(kind: &str) -> Self {
        if kind == "vehicle" { ModelDetail::VehicleExterior } else { ModelDetail::Item }
    }

    fn options(self) -> ExportOptions {
        let mut o = ExportOptions { material_mode: MaterialMode::Textures, ..ExportOptions::default() };
        match self {
            ModelDetail::Item => {
                o.lod_level = 0;
                o.texture_mip = 1;
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
        let opts = detail.options();
        let result = starbreaker_3d::assemble_glb_with_loadout_with_progress(&db, &self.p4k, &record, &tree, &opts, None, None, None, None)
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
