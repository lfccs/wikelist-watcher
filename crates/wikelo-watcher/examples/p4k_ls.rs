//! Diagnostic: what the archive holds under a path, to find static texture sets worth
//! turning into item images (images.rs). Prints a histogram of directories that hold
//! `.dds` / `.svg` / `.png` entries under `\ui\`, then the entries matching each word given.
//!
//! `cargo run --manifest-path watcher/Cargo.toml -p wikelo-watcher --example p4k_ls -- <Data.p4k> [word…]`

use std::collections::BTreeMap;

use starbreaker_p4k::MappedP4k;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: p4k_ls <Data.p4k> [word…]");
    let words: Vec<String> = args.map(|w| w.to_ascii_lowercase()).collect();
    let p4k = MappedP4k::open(&path).expect("open Data.p4k");
    let entries = p4k.entries();
    println!("# {} entries", entries.len());

    let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
    for e in entries {
        let lower = e.name.to_ascii_lowercase();
        if !lower.contains("\\ui\\") {
            continue;
        }
        if !(lower.ends_with(".dds") || lower.ends_with(".svg") || lower.ends_with(".png")) {
            continue;
        }
        let dir = match lower.rsplit_once('\\') {
            Some((d, _)) => d.to_string(),
            None => continue,
        };
        // Split mips (`.dds.1`) are not counted; they end with digits.
        *dirs.entry(dir).or_insert(0) += 1;
    }
    println!("\n# texture directories under \\ui\\ (count of dds/svg/png)");
    for (d, n) in &dirs {
        if *n >= 5 {
            println!("{n:>6}  {d}");
        }
    }

    for w in &words {
        println!("\n# entries containing {w:?}");
        let mut shown = 0;
        let mut total = 0;
        for e in entries {
            let lower = e.name.to_ascii_lowercase();
            if !lower.contains(w) {
                continue;
            }
            total += 1;
            if shown < 1200 {
                println!("  {}", e.name);
                shown += 1;
            }
        }
        println!("  ({total} total)");
    }
}
