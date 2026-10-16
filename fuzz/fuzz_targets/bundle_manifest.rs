//! Bundle manifest JSON and zip entry names (the first things the server reads from an upload).
#![no_main]

use libfuzzer_sys::fuzz_target;
use wikelo_watcher::bundle::{Manifest, safe_entry};

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<Manifest>(data);
    if let Ok(name) = std::str::from_utf8(data) {
        let _ = safe_entry(name);
    }
});
