// Compile the exact private recoder tests from the dependency, without adding
// a second workspace/dependency tree or exposing internal digits as public API.
extern crate std as ark_std;
mod pasta { pub use mina_curves::pasta::canonical_words; pub use mina_curves::pasta::{Fp,Fq,Pallas,Vesta,PallasParameters,VestaParameters}; }
#[path = "../../../vendor/proof-systems/curves/src/pasta/point_spikes.rs"]
mod point_spikes;
