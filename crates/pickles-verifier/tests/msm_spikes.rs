//! Native equivalence checks for the exact serial MSM experiment used by SP1.
//! No emulator or proving is invoked by this test target.

use ark_ec::{
    scalar_mul::variable_base::{msm_bigint_serial, msm_bigint_serial_with_config},
    AffineRepr, CurveGroup, VariableBaseMSM,
};
use ark_ff::{AdditiveGroup, Field, PrimeField, Zero};
use mina_curves::pasta::{Pallas, Vesta};

fn check_curve<C>()
where
    C: AffineRepr,
    C::Group: VariableBaseMSM<MulBase = C>,
    C::ScalarField: PrimeField,
{
    let bases: Vec<C> = (0..257)
        .map(|i| (C::generator() * C::ScalarField::from(i as u64 + 2)).into_affine())
        .collect();
    let scalars: Vec<C::ScalarField> = (0..257)
        .map(|i| {
            let mut s = C::ScalarField::from(i as u64 + 11);
            for _ in 0..5 {
                s.square_in_place();
                s += C::ScalarField::from(7u64);
            }
            match i % 11 {
                0 => C::ScalarField::ZERO,
                1 => C::ScalarField::ONE,
                // Near-modulus coefficients exercise signed digits and carry
                // handling in the last window, unlike small-only scalars.
                2 => -C::ScalarField::ONE,
                3 => -C::ScalarField::from(2u64),
                _ => s,
            }
        })
        .collect();
    let bigints: Vec<_> = scalars.iter().map(|s| s.into_bigint()).collect();

    // Independent scalar multiplication is the oracle, not another MSM path.
    for len in [0, 1, 31, 32, 257] {
        let expected = bases[..len]
            .iter()
            .zip(&scalars[..len])
            .fold(C::Group::zero(), |sum, (base, scalar)| {
                sum + (*base * scalar)
            });
        assert_eq!(
            msm_bigint_serial::<C::Group>(&bases[..len], &bigints[..len]),
            expected
        );
        for window_bits in [None, Some(10), Some(12), Some(14)] {
            for filter_zero in [false, true] {
                assert_eq!(
                    msm_bigint_serial_with_config::<C::Group>(
                        &bases[..len],
                        &bigints[..len],
                        window_bits,
                        filter_zero,
                    ),
                    expected,
                    "len={len}, window={window_bits:?}, filter={filter_zero}"
                );
            }
        }
    }

    // Pallas IPA's zero coefficients occupy a middle region: proof-dependent
    // nonzero points follow them. This catches an incorrect tail-only filter.
    let mut middle_zeros = scalars.clone();
    middle_zeros[32..240].fill(C::ScalarField::ZERO);
    let middle_bigints: Vec<_> = middle_zeros.iter().map(|s| s.into_bigint()).collect();
    let expected = bases
        .iter()
        .zip(&middle_zeros)
        .fold(C::Group::zero(), |sum, (base, scalar)| {
            sum + (*base * scalar)
        });
    for window_bits in [None, Some(10), Some(12), Some(14)] {
        assert_eq!(
            msm_bigint_serial_with_config::<C::Group>(&bases, &middle_bigints, window_bits, true),
            expected
        );
    }

    // Removing every term must produce the identity, and the unchecked helper
    // must retain its pre-existing semantics for unequal slice lengths.
    let zeros = vec![C::ScalarField::ZERO.into_bigint(); bases.len()];
    assert!(msm_bigint_serial_with_config::<C::Group>(&bases, &zeros, Some(12), true).is_zero());
    let expected_one = bases[1] * scalars[1];
    assert_eq!(
        msm_bigint_serial_with_config::<C::Group>(&bases[..2], &bigints, Some(10), true),
        expected_one
    );
    assert_eq!(
        msm_bigint_serial_with_config::<C::Group>(&bases, &bigints[..2], Some(14), true),
        expected_one
    );
}

#[test]
fn pallas_windows_and_zero_filter_match_naive_msm() {
    check_curve::<Pallas>();
}

#[test]
fn vesta_windows_and_zero_filter_match_naive_msm() {
    check_curve::<Vesta>();
}

#[test]
#[should_panic(expected = "MSM window must be 2..=16 bits")]
fn invalid_window_is_rejected() {
    let _ = msm_bigint_serial_with_config::<<Vesta as AffineRepr>::Group>(&[], &[], Some(0), false);
}
