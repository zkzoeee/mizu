//! Build the guest ELF, and tell cargo everything that should invalidate it.
//!
//! The bare three-line version of this script was a trap. It declared nothing,
//! so cargo reran it only when this crate's own sources changed -- which meant
//! `VK_JSON=... make build-rust` from a warm tree never called `sp1_build` at
//! all. It exited 0 in 1.6 s and left the previous guest in place, so you got a
//! guest built against the wrong VK with no error anywhere. The Makefile
//! advertises that override, so the documented path was the broken one.
//!
//! `GUEST_BUILD_ENV` is the shared list of variables that change what the guest
//! compiles to; the guest's own build script declares the same set.

include!("../o1-verifier/build-env.rs");

use std::path::PathBuf;

fn main() {
    for name in GUEST_BUILD_ENV {
        println!("cargo::rerun-if-env-changed={name}");
    }

    // The guest's sources, so editing the guest rebuilds it even though it is
    // not a dependency of this crate in cargo's eyes.
    for path in ["../o1-verifier/src", "../o1-verifier/build.rs", "../o1-verifier/Cargo.toml", "../o1-verifier/build-env.rs"] {
        println!("cargo::rerun-if-changed={path}");
    }

    // The baked VK itself. Same resolution order as the guest's build script,
    // so a changed file at the same path is caught as well as a changed path.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vk = std::env::var_os("VK_JSON")
        .or_else(|| std::env::var_os("SETTLEMENT_VK_JSON"))
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../fixtures/mainnet-blockchain-snark/vk.serde.json"));
    println!("cargo::rerun-if-changed={}", vk.display());

    sp1_build::build_program_with_args("../o1-verifier", Default::default());
}
