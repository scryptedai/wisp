//! Ensure the man page ships with every `wisp-cli` build.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let man_src = manifest_dir.join("../../contrib/man/wisp.1");
    if !man_src.is_file() {
        panic!(
            "missing {} — add contrib/man/wisp.1 (see .cursor/rules/docs-cli-man.mdc)",
            man_src.display()
        );
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let man_out = out_dir.join("wisp.1");
    fs::copy(&man_src, &man_out).expect("copy man page to OUT_DIR");
    println!("cargo:rerun-if-changed={}", man_src.display());
    let _ = man_out; // copied for OUT_DIR packagers; see package.metadata.wisp-install.
}
