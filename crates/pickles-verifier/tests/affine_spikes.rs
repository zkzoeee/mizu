//! Differential tests for the generic batched-affine MSM experiment.

use ark_ec::{
    scalar_mul::variable_base::affine_spike::msm_affine,
    short_weierstrass::{Affine, Projective, SWCurveConfig},
    AffineRepr, CurveGroup,
};
use ark_ff::{AdditiveGroup, BigInteger, Field, MontFp, PrimeField, Zero};
use mina_curves::pasta::{Fp, PallasParameters, VestaParameters};

fn naive<C: SWCurveConfig>(
    bases: &[Affine<C>],
    scalars: &[<C::ScalarField as PrimeField>::BigInt],
) -> Projective<C> {
    bases
        .iter()
        .zip(scalars)
        .fold(Projective::<C>::zero(), |sum, (base, scalar)| {
            sum + base.mul_bigint(*scalar)
        })
}

fn check_curve<C: SWCurveConfig>() {
    let generator = Affine::<C>::generator();
    let bases: Vec<_> = (0..257)
        .map(|i| match i % 7 {
            0 => Affine::<C>::identity(),
            1 | 2 => generator,
            3 => -generator,
            _ => (generator * C::ScalarField::from(i as u64 + 1)).into_affine(),
        })
        .collect();
    let scalars: Vec<_> = (0..257)
        .map(|i| {
            let value = match i % 7 {
                0 => C::ScalarField::ZERO,
                1 => C::ScalarField::ONE,
                2 => -C::ScalarField::ONE,
                3 => -C::ScalarField::from(2u64),
                _ => C::ScalarField::from(i as u64 + 17).pow([97]),
            };
            value.into_bigint()
        })
        .collect();
    let bits = C::ScalarField::MODULUS_BIT_SIZE as usize;
    for len in [0, 1, 2, 3, 31, 32, 257] {
        let expected = naive(&bases[..len], &scalars[..len]);
        for width in [3, 8, 10, 13] {
            assert_eq!(
                msm_affine(&bases[..len], &scalars[..len], width, bits),
                expected,
                "len={len}, width={width}"
            );
        }
    }

    // Every deposit lands in one bucket, including identity, doubling, and
    // cancellation pairs. Identities arising in early levels must remain safe
    // when combined with live points in later reduction levels.
    let adversarial: Vec<_> = (0..1025)
        .map(|i| match i % 8 {
            0 | 1 => generator,
            2 | 3 => -generator,
            4 => Affine::<C>::identity(),
            5 => (generator * C::ScalarField::from(2u64)).into_affine(),
            6 => -(generator * C::ScalarField::from(2u64)).into_affine(),
            _ => generator,
        })
        .collect();
    let ones = vec![C::ScalarField::ONE.into_bigint(); adversarial.len()];
    assert_eq!(
        msm_affine(&adversarial, &ones, 10, bits),
        naive(&adversarial, &ones)
    );
    let cancelling = [generator, generator, -generator, -generator];
    assert!(msm_affine(&cancelling, &ones[..4], 8, bits).is_zero());

    // Scalars are integers here, so all-one bitstrings may exceed the field
    // modulus. The independent oracle also multiplies by the integer bigint.
    // Include limb boundaries, partial high windows and a full 256-bit carry.
    for scalar_bits in [1, 4, 63, 64, 65, 127, 128, 129, 254, 255, 256] {
        let all_ones =
            <C::ScalarField as PrimeField>::BigInt::from_bits_le(&vec![true; scalar_bits]);
        let mut top_only = vec![false; scalar_bits];
        top_only[scalar_bits - 1] = true;
        let top_only = <C::ScalarField as PrimeField>::BigInt::from_bits_le(&top_only);
        let carry_scalars = [all_ones, top_only, all_ones];
        let carry_bases = [generator, -generator, generator];
        let expected = naive(&carry_bases, &carry_scalars);
        for width in [2, 5, 8, 10, 13, 16] {
            assert_eq!(
                msm_affine(&carry_bases, &carry_scalars, width, scalar_bits),
                expected,
                "scalar_bits={scalar_bits}, width={width}"
            );
        }
    }

    let zeros = vec![C::ScalarField::ZERO.into_bigint(); bases.len()];
    assert!(msm_affine(&bases, &zeros, 10, 0).is_zero());
    assert_eq!(
        msm_affine(&bases[..32], &scalars, 8, bits),
        naive(&bases[..32], &scalars)
    );
    assert_eq!(
        msm_affine(&bases, &scalars[..31], 8, bits),
        naive(&bases, &scalars[..31])
    );
    // An unpaired scalar is outside the shorter-slice contract and is ignored,
    // including by the bit-bound validation.
    assert!(msm_affine::<C>(&[], &scalars, 8, 0).is_zero());
}

#[test]
fn pallas_affine_msm_matches_independent_scalar_multiplication() {
    check_curve::<PallasParameters>();
}

#[test]
fn vesta_affine_msm_matches_independent_scalar_multiplication() {
    check_curve::<VestaParameters>();
}

// An arithmetic-only test curve with nonzero A and a point of order two.
// Its scalar-field/cofactor declarations are not used as subgroup claims.
struct ArithmeticTestCurve;

impl ark_ec::CurveConfig for ArithmeticTestCurve {
    type BaseField = Fp;
    type ScalarField = Fp;
    const COFACTOR: &'static [u64] = &[1];
    const COFACTOR_INV: Fp = MontFp!("1");
}

impl SWCurveConfig for ArithmeticTestCurve {
    const COEFF_A: Fp = MontFp!("2");
    const COEFF_B: Fp = MontFp!("3");
    const GENERATOR: Affine<Self> = Affine::new_unchecked(MontFp!("3"), MontFp!("6"));
}

#[test]
fn nonzero_a_and_zero_y_doubling_are_complete() {
    let g = Affine::<ArithmeticTestCurve>::generator();
    let order_two = Affine::<ArithmeticTestCurve>::new_unchecked(-Fp::ONE, Fp::ZERO);
    assert!(g.is_on_curve());
    assert!(order_two.is_on_curve());
    let bases = [
        order_two,
        order_two,
        g,
        g,
        -g,
        Affine::<ArithmeticTestCurve>::identity(),
        order_two,
    ];
    let scalars = [Fp::ONE.into_bigint(); 7];
    assert_eq!(msm_affine(&bases, &scalars, 3, 1), naive(&bases, &scalars));
    for scalar in [1u64, 2, 3, 7, 8, 15, 16, 31, 255] {
        let scalars = [Fp::from(scalar).into_bigint(); 7];
        assert_eq!(msm_affine(&bases, &scalars, 3, 8), naive(&bases, &scalars));
    }
}

#[test]
#[should_panic(expected = "affine MSM scalar exceeds declared bit bound")]
fn affine_msm_rejects_truncated_scalar() {
    let scalar = <VestaParameters as ark_ec::CurveConfig>::ScalarField::from(256u64).into_bigint();
    let _ = msm_affine(&[Affine::<VestaParameters>::generator()], &[scalar], 8, 8);
}

#[test]
#[should_panic(expected = "affine MSM window must be 2..=16 bits")]
fn affine_msm_rejects_invalid_window() {
    let _ = msm_affine::<VestaParameters>(&[], &[], 17, 255);
}
