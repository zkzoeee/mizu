// Compile the vendored arithmetic tests in a runnable workspace test target.
// The native library keeps the ordinary permutation, providing an independent
// reference for every canonical round and parameter fallback.
pub use mina_poseidon::{constants, pasta, poseidon};
#[path = "../../../vendor/proof-systems/poseidon/src/canonical.rs"]
mod canonical;
