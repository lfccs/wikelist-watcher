//! Diagnostic: does a DataCore record carry any picture reference? Materializes the records
//! whose name contains a word and prints every JSON path whose key or string value contains
//! one of the needles (icon, preview, thumb, render, .dds, .svg…), plus the component type
//! names found, to see where the game would take an inventory icon from.
//!
//! `cargo run --manifest-path watcher/Cargo.toml -p wikelo-watcher --example dcb_find -- <Data.p4k> <record-word> [needle…]`

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;
use starbreaker_datacore::Database;
use wikelo_watcher::GameData;
use wikelo_watcher::resolver::{RefResolver, record_value};

fn walk(v: &Value, path: &str, needles: &[String], hits: &mut Vec<String>, types: &mut BTreeSet<String>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                let p = format!("{path}.{k}");
                let kl = k.to_ascii_lowercase();
                if (kl == "__type" || kl == "type" || kl == "__polymorphictype") && x.is_string() {
                    types.insert(x.as_str().unwrap_or("").to_string());
                }
                if needles.iter().any(|n| kl.contains(n)) {
                    let s = serde_json::to_string(x).unwrap_or_default();
                    hits.push(format!("{p} = {}", &s[..s.len().min(160)]));
                }
                walk(x, &p, needles, hits, types);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                walk(x, &format!("{path}[{i}]"), needles, hits, types);
            }
        }
        Value::String(s) => {
            let sl = s.to_ascii_lowercase();
            if needles.iter().any(|n| sl.contains(n)) {
                hits.push(format!("{path} = {:?}", &s[..s.len().min(160)]));
            }
        }
        _ => {}
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let p4k = PathBuf::from(args.next().expect("usage: dcb_find <Data.p4k> <record-word> [needle…]"));
    let word = args.next().unwrap_or_default().to_ascii_lowercase();
    let mut needles: Vec<String> = args.map(|n| n.to_ascii_lowercase()).collect();
    if needles.is_empty() {
        needles = ["icon", "preview", "thumb", "render", ".dds", ".svg", "image"].iter().map(|s| s.to_string()).collect();
    }
    let data = GameData::open(&p4k, None).expect("open Data.p4k");
    let db = Database::from_bytes(&data.dcb).expect("parse Game2.dcb");
    let mut res = RefResolver::new(&db);
    let mut shown = 0;
    for r in db.records() {
        let name = db.resolve_string2(r.name_offset);
        if !name.to_ascii_lowercase().contains(&word) {
            continue;
        }
        shown += 1;
        if shown > 6 {
            println!("…");
            break;
        }
        println!("\n## {} ({})", name, db.struct_name(r.struct_id()));
        let Ok(v) = res.materialize(r) else {
            println!("  (cannot materialize)");
            continue;
        };
        let root = record_value(&v);
        let mut hits = Vec::new();
        let mut types = BTreeSet::new();
        walk(&root, "", &needles, &mut hits, &mut types);
        let json = serde_json::to_string(&*root).unwrap_or_default();
        println!("  json size {} bytes · component types: {}", json.len(), types.iter().cloned().collect::<Vec<_>>().join(", "));
        for h in hits.iter().take(40) {
            println!("  {h}");
        }
        if hits.len() > 40 {
            println!("  … {} more", hits.len() - 40);
        }
    }
}
