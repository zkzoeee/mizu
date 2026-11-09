//! Differential arithmetic checks for the executor-only Pasta point backend.
use ark_ec::{
    short_weierstrass::{Affine, Projective, SWCurveConfig},
    AffineRepr, CurveGroup,
};
use ark_ff::{AdditiveGroup, BigInt, BigInteger, Field, PrimeField, Zero};
use mina_curves::pasta::{
    canonical_pallas::{msm_bigint_points_with_config, MsmSpikeConfig},
    point_spikes::{self, PastaCurve},
    Fp, Fq, Pallas, PallasParameters, VestaParameters,
};

fn reference<C: SWCurveConfig>(bases: &[Affine<C>], scalars: &[BigInt<4>]) -> Projective<C> {
    bases
        .iter()
        .zip(scalars)
        .fold(Projective::zero(), |sum, (base, scalar)| {
            // Independent ordinary single-point double-and-add; no MSM dispatch.
            sum + base.mul_bigint(*scalar)
        })
}

fn exceptional_points<C: SWCurveConfig>(curve: PastaCurve)
where
    C::BaseField: PrimeField<BigInt = BigInt<4>>,
    C::ScalarField: PrimeField<BigInt = BigInt<4>>,
{
    let g = Affine::<C>::generator();
    let two = (g + g).into_affine();
    let zero = Affine::<C>::identity();
    for a in [zero, g, -g, two, -two] {
        for b in [zero, g, -g, two, -two] {
            assert_eq!(point_spikes::add(&a, &b, curve), (a + b).into_affine());
        }
        assert_eq!(point_spikes::double(&a, curve), (a + a).into_affine());
    }
    // Prime-order Pasta has no nonidentity order-two point. This synthetic
    // unchecked input tests only the zero-denominator guard, not provenance.
    let zero_y = Affine::<C>::new_unchecked(g.x, C::BaseField::ZERO);
    assert!(point_spikes::double(&zero_y, curve).is_zero());
    assert!(point_spikes::add(&zero_y, &zero_y, curve).is_zero());
}

#[test]
fn both_curves_have_complete_exceptional_point_wrappers() {
    exceptional_points::<PallasParameters>(PastaCurve::Pallas);
    exceptional_points::<VestaParameters>(PastaCurve::Vesta);
}

fn msm_cases<C: SWCurveConfig>(curve: PastaCurve)
where
    C::BaseField: PrimeField<BigInt = BigInt<4>>,
    C::ScalarField: PrimeField<BigInt = BigInt<4>>,
{
    let g = Affine::<C>::generator();
    let bases = [Affine::identity(), g, -g, g, (g + g).into_affine(), g];
    for bits in [0, 1, 63, 64, 65, 127, 128, 129, 255, 256] {
        let mut full = BigInt([u64::MAX; 4]);
        for bit in bits..256 {
            full.0[bit / 64] &= !(1 << (bit % 64));
        }
        let mut near = full;
        if bits != 0 {
            near.sub_with_borrow(&BigInt([1, 0, 0, 0]));
        }
        let one = if bits == 0 {
            BigInt([0; 4])
        } else {
            BigInt([1, 0, 0, 0])
        };
        let scalars = [full, near, one, full, BigInt([0; 4]), near];
        let expected = reference(&bases, &scalars);
        for width in [2, 3, 5, 8] {
            assert_eq!(
                point_spikes::msm(&bases, &scalars, width, bits, point_spikes::CarryWindow::Reserved, curve),
                expected,
                "curve={curve:?} bits={bits} width={width}"
            );
        }
    }
    let scalars = [BigInt([0; 4]), BigInt([15, 0, 0, 0]), BigInt([16, 0, 0, 0])];
    for base_len in [0, 1, 2, 6] {
        for scalar_len in 0..=scalars.len() {
            assert_eq!(
                point_spikes::msm(&bases[..base_len], &scalars[..scalar_len], 3, 5, point_spikes::CarryWindow::Reserved, curve),
                reference(&bases[..base_len], &scalars[..scalar_len]),
            );
        }
    }
    // Every allowed window, including16, exercises carry out of a full window.
    for width in 2..=16 {
        let scalars = [BigInt([(1 << width) - 1, 0, 0, 0])];
        assert_eq!(
            point_spikes::msm(&[g], &scalars, width, width, point_spikes::CarryWindow::Reserved, curve),
            reference(&[g], &scalars)
        );
    }
}

#[test]
fn both_curves_match_full_width_and_shortened_scalar_oracles() {
    msm_cases::<PallasParameters>(PastaCurve::Pallas);
    msm_cases::<VestaParameters>(PastaCurve::Vesta);
}

#[test]
fn canonical_pallas_point_schedule_preserves_glv_and_zero_filter() {
    let g = Pallas::generator();
    let bases = [g, -g, Pallas::zero(), g, (g * Fq::from(7)).into_affine()];
    let scalars = [
        Fq::MODULUS,
        BigInt([u64::MAX; 4]),
        BigInt([0; 4]),
        (-Fq::ONE).into_bigint(),
        Fq::ONE.into_bigint(),
    ];
    let expected = reference(&bases, &scalars);
    for glv in [false, true] {
        for trim_conversions in [false, true] {
            let config = MsmSpikeConfig {
                window_bits: Some(5),
                filter_zero: true,
                trim_conversions,
                glv,
                ..Default::default()
            };
            assert_eq!(
                msm_bigint_points_with_config(&bases, &scalars, config),
                expected
            );
            assert_eq!(
                msm_bigint_points_with_config(&bases[..3], &scalars[..2], config),
                reference(&bases[..3], &scalars[..2])
            );
        }
    }
}

#[test]
fn point_msm_keeps_cancellation_equations_and_detects_mutation() {
    let g = Pallas::generator();
    let bases = [g, g, -g];
    let mut scalars = [Fq::from(13), Fq::from(19), Fq::from(32)].map(PrimeField::into_bigint);
    assert!(point_spikes::msm(&bases, &scalars, 5, 6, point_spikes::CarryWindow::Reserved, PastaCurve::Pallas).is_zero());
    scalars[0] = Fq::from(14).into_bigint();
    assert_eq!(
        point_spikes::msm(&bases, &scalars, 5, 6, point_spikes::CarryWindow::Reserved, PastaCurve::Pallas),
        g.into_group()
    );
}

#[test]
#[should_panic(expected = "point MSM scalar exceeds declared bit bound")]
fn point_msm_rejects_scalar_truncation() {
    point_spikes::msm(
        &[Pallas::generator()],
        &[BigInt([256, 0, 0, 0])],
        3,
        8,
        point_spikes::CarryWindow::Reserved, PastaCurve::Pallas,
    );
}

#[test]
#[should_panic(expected = "point MSM window must be 2..=16")]
fn point_msm_rejects_invalid_window() {
    point_spikes::msm(
        &[Pallas::generator()],
        &[Fq::ONE.into_bigint()],
        17,
        255,
        point_spikes::CarryWindow::Reserved, PastaCurve::Pallas,
    );
}

#[test]
#[should_panic(expected = "point backend base field")]
fn point_backend_rejects_swapped_pasta_moduli() {
    point_spikes::double(&Pallas::generator(), PastaCurve::Vesta);
}

#[test]
#[should_panic(expected = "equal-x points are not opposites")]
fn point_backend_rejects_malformed_equal_x_pair() {
    let g = Pallas::generator();
    let invalid = Pallas::new_unchecked(g.x, g.y + Fp::ONE);
    point_spikes::add(&g, &invalid, PastaCurve::Pallas);
}
