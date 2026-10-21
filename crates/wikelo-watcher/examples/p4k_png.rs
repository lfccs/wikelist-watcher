//! Diagnostic: convert one DDS / SVG entry of the archive to a file, to eyeball a texture
//! set before teaching images.rs about it.
//!
//! `cargo run --manifest-path watcher/Cargo.toml -p wikelo-watcher --example p4k_png -- <Data.p4k> <entry> <out.png|svg>`

use starbreaker_p4k::MappedP4k;
use wikelo_watcher::images::{ImageSource, ImageSourceKind, render};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: p4k_png <Data.p4k> <entry> <out>");
    let entry = args.next().expect("entry path inside the archive");
    let out = args.next().expect("output file");
    let p4k = MappedP4k::open(&path).expect("open Data.p4k");
    let src = ImageSource { entry, kind: ImageSourceKind::Generic };
    let bytes = render(&p4k, &src).expect("render");
    std::fs::write(&out, &bytes).expect("write");
    println!("{out}: {} bytes", bytes.len());
}
