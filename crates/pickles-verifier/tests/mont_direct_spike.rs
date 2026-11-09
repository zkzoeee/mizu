//! Validate the exact field selector and Montgomery correction constants.
//! Real direct-syscall coverage comes from the separately executed guest ELFs.

use ark_ff::{pasta_direct_mulmod_correction, BigInt, PrimeField};
use mina_curves::pasta::{Fp, Fq};

fn check_correction<F: PrimeField<BigInt = BigInt<4>>>() {
    let operands = pasta_direct_mulmod_correction(&F::MODULUS).expect("Pasta modulus");
    assert_eq!(operands[1], F::MODULUS.0);
    let r = F::from_bigint(F::MODULUS.montgomery_r()).unwrap();
    let r_inverse = F::from_bigint(BigInt(operands[0])).expect("canonical R inverse");
    assert_eq!(r * r_inverse, F::ONE);
    assert_ne!(
        r,
        F::ONE,
        "omitting R correction must change nonzero products"
    );

    // Full-modulus matching must reject unrelated fields sharing the same
    // low limb. Also reject alterations to that low limb and limb-count aliases.
    for limb in 0..4 {
        let mut changed = F::MODULUS;
        changed.0[limb] ^= 2;
        assert!(pasta_direct_mulmod_correction(&changed).is_none());
    }
    assert!(pasta_direct_mulmod_correction(&BigInt::<3>([
        F::MODULUS.0[0],
        F::MODULUS.0[1],
        F::MODULUS.0[2],
    ]))
    .is_none());
    assert!(pasta_direct_mulmod_correction(&BigInt::<5>([
        F::MODULUS.0[0],
        F::MODULUS.0[1],
        F::MODULUS.0[2],
        F::MODULUS.0[3],
        0,
    ]))
    .is_none());

    // The syscall reads a full contiguous 64-byte second operand even though
    // its ABI argument type points to only the first [u64; 4].
    assert_eq!(core::mem::size_of_val(operands), 64);
    assert_eq!(core::mem::align_of_val(operands), 8);
    assert_eq!(
        operands[1].as_ptr() as usize - operands[0].as_ptr() as usize,
        32
    );
}

#[test]
fn both_pasta_corrections_are_exact_and_other_moduli_are_rejected() {
    check_correction::<Fp>();
    check_correction::<Fq>();
    assert!(pasta_direct_mulmod_correction(&BigInt::<0>([])).is_none());
    assert!(pasta_direct_mulmod_correction(&BigInt::<4>([0; 4])).is_none());
}
