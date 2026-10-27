// SPDX-License-Identifier: MPL-2.0
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
fn sources(path: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(path).expect("source directory") {
        let p = entry.expect("source entry").path();
        if p.is_dir() {
            sources(&p, files);
        } else if p.extension().is_some_and(|v| v == "rs") {
            files.push(p);
        }
    }
}
fn main() {
    let mut files = vec![
        PathBuf::from("Cargo.toml"),
        PathBuf::from("Cargo.lock"),
        PathBuf::from("uniffi.toml"),
        PathBuf::from("build.rs"),
    ];
    sources(Path::new("src"), &mut files);
    files.sort();
    let mut hash = Sha256::new();
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        let name = file.to_string_lossy().replace('\\', "/");
        hash.update(name.as_bytes());
        hash.update([0]);
        let data = fs::read(file).expect("source file");
        hash.update((data.len() as u64).to_be_bytes());
        hash.update(data);
    }
    let id: String = hash.finalize().iter().map(|v| format!("{v:02x}")).collect();
    println!("cargo:rustc-env=LIBOPENZILO_BUILD_ID={id}");
}
