//! Run the dependency's exact canonical-prefix helper and its independent tests.
mod commitment { pub use poly_commitment::commitment::b_poly_coefficients; }
#[path = "../../../vendor/proof-systems/poly-commitment/src/canonical_ipa.rs"]
mod canonical_ipa;
