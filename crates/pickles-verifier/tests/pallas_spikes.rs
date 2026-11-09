//! Differential checks for the isolated outer IPA canonical Pallas backend.

use ark_ec::{AffineRepr, CurveGroup, VariableBaseMSM};
use ark_ff::{AdditiveGroup, BigInt, Field, PrimeField, Zero};
use ark_serialize::CanonicalSerialize;
use mina_curves::pasta::{canonical_pallas, Fp, Fq, Pallas, ProjectivePallas, Vesta};
use poly_commitment::commitment::CommitmentCurve;

fn reference(bases: &[Pallas], scalars: &[Fq]) -> ProjectivePallas {
    // Independent double-and-add point multiplication, without an MSM backend.
    bases
        .iter()
        .zip(scalars)
        .fold(ProjectivePallas::zero(), |sum, (base, scalar)| {
            sum + *base * scalar
        })
}

#[test]
fn canonical_pallas_matches_reference_with_exceptional_points_and_scalars() {
    let generator = Pallas::generator();
    for len in [0, 1, 2, 31, 32, 257, 1024] {
        let bases: Vec<_> = (0..len)
            .map(|i| match i % 7 {
                0 => Pallas::zero(),
                1 | 2 => generator,
                3 => -generator,
                _ => (generator * Fq::from(i as u64 + 1)).into_affine(),
            })
            .collect();
        let scalars: Vec<_> = (0..len)
            .map(|i| match i % 7 {
                0 | 1 => Fq::ZERO,
                2 => Fq::ONE,
                3 => -Fq::ONE,
                4 => Fq::from(2).pow([254]),
                _ => Fq::from(i as u64 + 9).pow([17]),
            })
            .collect();
        let bigints: Vec<_> = scalars.iter().map(|scalar| scalar.into_bigint()).collect();
        let expected = reference(&bases, &scalars);
        let actual = canonical_pallas::msm_bigint(&bases, &bigints);
        assert_eq!(actual, expected, "length {len}");
        assert_eq!(
            Pallas::verifier_msm_bigint(&bases, &bigints),
            expected,
            "typed verifier hook, length {len}"
        );

        // The backend's internal representation cannot escape at serialization.
        let mut actual_bytes = Vec::new();
        let mut expected_bytes = Vec::new();
        actual
            .into_affine()
            .serialize_compressed(&mut actual_bytes)
            .unwrap();
        expected
            .into_affine()
            .serialize_compressed(&mut expected_bytes)
            .unwrap();
        assert_eq!(actual_bytes, expected_bytes);
    }
}

#[test]
fn canonical_pallas_preserves_shorter_slice_semantics() {
    let generator = Pallas::generator();
    let bases = [generator, -generator, Pallas::zero()];
    let scalars = [Fq::ONE, Fq::from(19), -Fq::ONE];
    let bigints: Vec<_> = scalars.iter().map(|scalar| scalar.into_bigint()).collect();
    for base_len in 0..=bases.len() {
        for scalar_len in 0..=scalars.len() {
            let expected = reference(&bases[..base_len], &scalars[..scalar_len]);
            assert_eq!(
                canonical_pallas::msm_bigint(&bases[..base_len], &bigints[..scalar_len]),
                expected
            );
            assert_eq!(
                ProjectivePallas::msm_bigint(&bases[..base_len], &bigints[..scalar_len]),
                expected
            );
        }
    }
}

#[test]
fn verifier_hook_preserves_zero_equation_and_detects_changed_coefficient() {
    let generator = Pallas::generator();
    let bases = [generator, generator, -generator, Pallas::zero()];
    let mut scalars =
        [Fq::from(37), Fq::from(19), Fq::from(56), -Fq::ONE].map(PrimeField::into_bigint);
    assert!(Pallas::verifier_msm_bigint(&bases, &scalars).is_zero());
    assert!(canonical_pallas::msm_bigint(&bases, &scalars).is_zero());
    scalars[0] = Fq::from(38).into_bigint();
    assert_eq!(
        Pallas::verifier_msm_bigint(&bases, &scalars),
        generator.into_group()
    );
    assert_eq!(
        canonical_pallas::msm_bigint(&bases, &scalars),
        generator.into_group()
    );

    // Other curve configurations retain the default hook even when the
    // canonical Pallas experiment is compiled in.
    let vesta = Vesta::generator();
    let vesta_scalars: [BigInt<4>; 2] = [Fp::from(2), Fp::ONE].map(PrimeField::into_bigint);
    assert_eq!(
        Vesta::verifier_msm_bigint(&[vesta, -vesta], &vesta_scalars),
        vesta.into_group()
    );
}
