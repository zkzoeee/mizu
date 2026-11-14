//! Copy an explicitly selected prepared SRS table into the guest's build inputs.

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo::rerun-if-env-changed=MIZU_PREPARED_VESTA_PATH");
    let destination =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("vesta-prepared.bin");
    if let Some(source) = env::var_os("MIZU_PREPARED_VESTA_PATH") {
        let source = PathBuf::from(source);
        println!("cargo::rerun-if-changed={}", source.display());
        fs::copy(&source, &destination).unwrap_or_else(|error| {
            panic!("copy prepared Vesta table {}: {error}", source.display())
        });
    } else {
        // Truncate any previous table when the environment selection is removed.
        fs::write(&destination, []).expect("write empty vesta-prepared.bin");
    }
}
