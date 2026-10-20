//! Puts a fingerprint of the index source code in `RUBYSCOPE_INDEX_FINGERPRINT`.
//! A saved index from other code (other parse rules) is not used.

use std::path::Path;

fn main() {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut add = |bytes: &[u8]| {
        for b in bytes {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    let mut files = Vec::new();
    collect(Path::new("src"), &mut files);
    files.sort();
    files.push("../../Cargo.lock".into());
    files.push("Cargo.toml".into());
    for f in &files {
        if let Ok(bytes) = std::fs::read(f) {
            add(f.to_string_lossy().as_bytes());
            add(&bytes);
        }
        println!("cargo:rerun-if-changed={}", f.display());
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rustc-env=RUBYSCOPE_INDEX_FINGERPRINT={hash:016x}");
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}
