//! Diagnostic: which DataCore records mention a word, and how the matching contracts
//! inside `ContractGenerator` records pay out. Used to see why a mission seen in Game.log
//! has no reward in the dataset (new reward shapes, missions outside the generators…).
//!
//! `cargo run --manifest-path watcher/Cargo.toml -p wikelo-watcher --example dump_records -- <Data.p4k> <word>`

use std::collections::BTreeMap;
use std::path::PathBuf;

use starbreaker_datacore::Database;
use wikelo_watcher::GameData;
use wikelo_watcher::normalize::{arr, s, ty};
use wikelo_watcher::resolver::{RefResolver, record_value};

fn main() {
    let mut args = std::env::args().skip(1);
    let p4k = PathBuf::from(args.next().expect("usage: dump_records <Data.p4k> <word>"));
    let needle = args.next().unwrap_or_default().to_ascii_lowercase();
    let data = GameData::open(&p4k, None).expect("open Data.p4k");
    let db = Database::from_bytes(&data.dcb).expect("parse Game2.dcb");

    println!("# records whose name contains {needle:?}");
    let mut shown = 0;
    for r in db.records() {
        let name = db.resolve_string2(r.name_offset);
        if !needle.is_empty() && name.to_ascii_lowercase().contains(&needle) {
            println!("  {:<44} {name}", db.struct_name(r.struct_id()));
            shown += 1;
            if shown >= 80 {
                println!("  …");
                break;
            }
        }
    }

    println!("\n# contracts inside ContractGenerator records");
    let mut res = RefResolver::new(&db);
    let mut result_types: BTreeMap<String, usize> = BTreeMap::new();
    for (gname, rec) in res.records_of_type("ContractGenerator") {
        let Ok(v) = res.materialize(&rec) else { continue };
        let root = record_value(&v);
        if !needle.is_empty() && gname.to_ascii_lowercase().contains(&needle) {
            let keys: Vec<&String> = root.as_object().map(|o| o.keys().collect()).unwrap_or_default();
            println!("
## generator record {gname}: root keys {keys:?}");
            for (gi, g) in arr(&root, "generators").iter().enumerate() {
                if let Some(obj) = g.as_object() {
                    for (k, val) in obj {
                        if let Some(items) = val.as_array() {
                            let names: Vec<&str> = items.iter().take(6).map(|c| s(c, "debugName").or_else(|| s(c, "name")).unwrap_or("?")).collect();
                            println!("  generators[{gi}].{k}: {} entries, e.g. {names:?}", items.len());
                        } else {
                            println!("  generators[{gi}].{k}: {}", serde_json::to_string(val).unwrap_or_default().chars().take(120).collect::<String>());
                        }
                    }
                }
            }
        }
        for g in arr(&root, "generators") {
            for key in ["contracts", "introContracts", "legacyContracts"] {
                for c in arr(g, key) {
                    let dn = s(c, "debugName").unwrap_or("?");
                    let results: Vec<&serde_json::Value> = c.get("contractResults").map(|r| arr(r, "contractResults").iter().collect()).unwrap_or_default();
                    for r in &results {
                        *result_types.entry(ty(r).unwrap_or("?").to_string()).or_default() += 1;
                    }
                    if needle.is_empty() || !format!("{gname} {dn}").to_ascii_lowercase().contains(&needle) {
                        continue;
                    }
                    println!("\n== {gname} :: {dn} (id {})", s(c, "id").unwrap_or("?"));
                    if results.is_empty() {
                        let compact = serde_json::to_string(c).unwrap_or_default();
                        println!("  no contractResults array; entry: {}", &compact[..compact.len().min(5000)]);
                    }
                    for r in results {
                        let compact = serde_json::to_string(r).unwrap_or_default();
                        println!("  {}: {}", ty(r).unwrap_or("?"), &compact[..compact.len().min(900)]);
                    }
                }
            }
        }
    }
    println!("\n# contractResult types across every generator: {result_types:?}");
}
