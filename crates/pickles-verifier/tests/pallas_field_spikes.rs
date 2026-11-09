// Exercise the dependency's exact field implementation and inline unit tests
// without changing workspace membership or resolving a second dependency tree.
// Cargo cannot select an excluded dependency's own dev-dependency test target.
extern crate std as ark_std;

mod pasta {
    pub use mina_curves::pasta::pallas_glv;
    pub use mina_curves::pasta::point_spikes;
    pub use mina_curves::pasta::{Fp, Fq, Pallas, PallasParameters};
}

#[path = "../../../vendor/proof-systems/curves/src/pasta/canonical_pallas.rs"]
mod canonical_pallas;
