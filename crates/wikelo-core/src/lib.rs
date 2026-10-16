//! Domain types shared by every Wikelist crate: the normalized dataset
//! produced by the extractor and the pure computations the UI needs
//! (missing items, "can complete now", progress).

pub mod compute;
pub mod dataset;
pub mod history;
pub mod validate;

pub use compute::*;
pub use dataset::*;
pub use history::*;
pub use validate::*;

/// Git commit this workspace was built from (`dev` outside a checkout; see `build.rs`).
pub const GIT_COMMIT: &str = env!("WIKELO_GIT_COMMIT");
/// Version of the workspace crates.
pub const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
