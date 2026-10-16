//! JSON into `WikeloDataset`, then everything the server runs on an uploaded dataset: the
//! validator, the canonical hash, item usage and the contract evaluation.
#![no_main]

use libfuzzer_sys::fuzz_target;
use wikelo_core::{Inventory, WikeloDataset, canonical_hash, evaluate_with, item_usage, validate_dataset};

fuzz_target!(|data: &[u8]| {
    let Ok(ds) = serde_json::from_slice::<WikeloDataset>(data) else { return };
    let _ = validate_dataset(&ds, None);
    let _ = canonical_hash(&ds);
    let _ = item_usage(&ds);
    let inv = Inventory::default();
    for c in ds.contracts.iter().take(64) {
        let _ = evaluate_with(c, &inv, Some(0), &ds.crafting);
    }
});
