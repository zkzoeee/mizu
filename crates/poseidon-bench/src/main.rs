//! Small exact-Mina-Poseidon CPU benchmark, independent of the embedded SRS.
#![no_main]
sp1_zkvm::entrypoint!(main);
#[cfg(target_os = "zkvm")]
#[path = "../../o1-verifier/src/atomic_shims.rs"]
mod atomic_shims;

use ark_ff::PrimeField;
use mina_curves::pasta::{Fp, Fq};
use mina_poseidon::{
    constants::PlonkSpongeConstantsKimchi,
    pasta::{fp_kimchi, fq_kimchi},
    permutation::poseidon_block_cipher,
    poseidon::ArithmeticSpongeParams,
};

fn run<F: PrimeField>(params: &ArithmeticSpongeParams<F, 55>, iterations: u32) {
    let mut state = [F::ZERO, F::ONE, -F::ONE];
    for _ in 0..iterations {
        poseidon_block_cipher::<F, PlonkSpongeConstantsKimchi, 55>(params, &mut state);
    }
    for value in state {
        for limb in value.into_bigint().as_ref() {
            sp1_zkvm::io::commit_slice(&limb.to_le_bytes());
        }
    }
}

pub fn main() {
    let iterations = sp1_zkvm::io::read::<u32>();
    assert!((1..=64).contains(&iterations));
    sp1_zkvm::io::commit_slice(&iterations.to_le_bytes());
    sp1_zkvm::io::write(1, b"cycle-tracker-report-start:poseidon-fp\n");
    run::<Fp>(fp_kimchi::static_params(), iterations);
    sp1_zkvm::io::write(1, b"cycle-tracker-report-end:poseidon-fp\n");
    sp1_zkvm::io::write(1, b"cycle-tracker-report-start:poseidon-fq\n");
    run::<Fq>(fq_kimchi::static_params(), iterations);
    sp1_zkvm::io::write(1, b"cycle-tracker-report-end:poseidon-fq\n");
}
