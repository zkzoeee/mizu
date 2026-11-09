//! Differential tests for Pallas-only MSM experiments. No SP1 execution/proving.
use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::{AdditiveGroup, BigInt, BigInteger, Field, PrimeField, Zero};
use mina_curves::pasta::{
    canonical_pallas::{msm_bigint_with_config, MsmSpikeConfig},
    Fq, Pallas, ProjectivePallas,
};

// Run the exact dependency's inline fixed-limb arithmetic tests without adding
// a second workspace or resolving a separate dependency graph.
mod pasta {
    pub use mina_curves::pasta::{Fp, Fq, Pallas};
}
#[path = "../../../vendor/proof-systems/curves/src/pasta/pallas_glv.rs"]
mod pallas_glv;

fn reference(bases: &[Pallas], scalars: &[BigInt<4>]) -> ProjectivePallas {
    bases
        .iter()
        .zip(scalars)
        .fold(ProjectivePallas::zero(), |sum, (base, scalar)| {
            // Single-point multiplication covers all 256 integer bits, unlike
            // variable-base recoding schedules that assume canonical field inputs.
            sum + base.mul_bigint(*scalar)
        })
}

fn variants() -> [MsmSpikeConfig; 5] {
    [
        MsmSpikeConfig {
            filter_zero: true,
            ..Default::default()
        },
        MsmSpikeConfig {
            filter_zero: true,
            trim_conversions: true,
            ..Default::default()
        },
        MsmSpikeConfig {
            filter_zero: true,
            affine: true,
            ..Default::default()
        },
        MsmSpikeConfig {
            filter_zero: true,
            trim_conversions: true,
            affine: true,
            ..Default::default()
        },
        MsmSpikeConfig {
            filter_zero: true,
            trim_conversions: true,
            affine: true,
            glv: true,
            ..Default::default()
        },
    ]
}

#[test]
fn all_schedules_match_single_point_oracle_and_shorter_slices() {
    let generator = Pallas::generator();
    let mut state = Fq::from(513u64);
    let bases: Vec<_> = (0..129)
        .map(|i| match i % 7 {
            0 => Pallas::zero(),
            1 => generator,
            2 => -generator,
            _ => (generator * Fq::from(i as u64 + 1)).into_affine(),
        })
        .collect();
    let scalars: Vec<_> = (0..129)
        .map(|i| {
            state = state.square() + Fq::from(17u64);
            match i % 9 {
                0 => Fq::ZERO,
                1 => -Fq::ONE,
                _ => state,
            }
            .into_bigint()
        })
        .collect();
    for (base_len, scalar_len) in [
        (0, 0),
        (1, 0),
        (0, 1),
        (1, 1),
        (2, 2),
        (31, 32),
        (32, 31),
        (129, 129),
    ] {
        let bases = &bases[..base_len];
        let scalars = &scalars[..scalar_len];
        let expected = reference(bases, scalars);
        for mut config in variants() {
            for width in [None, Some(8)] {
                config.window_bits = width;
                assert_eq!(
                    msm_bigint_with_config(bases, scalars, config),
                    expected,
                    "base_len={base_len} scalar_len={scalar_len} config={config:?}"
                );
            }
        }
    }
}

#[test]
fn early_zero_filter_preserves_late_nonzero_terms_and_disabled_filter() {
    let generator = Pallas::generator();
    let bases = vec![generator; 1025];
    let mut scalars = vec![BigInt([0; 4]); bases.len()];
    for i in [0, 31, 512, 1024] {
        scalars[i] = Fq::from(i as u64 + 1).into_bigint();
    }
    let expected = reference(&bases, &scalars);
    for mut config in variants() {
        // This is the override that was selected from the original >=1024
        // length, even though filtering leaves four terms.
        config.window_bits = Some(12);
        assert_eq!(msm_bigint_with_config(&bases, &scalars, config), expected);
    }
    let disabled = MsmSpikeConfig {
        trim_conversions: true,
        ..Default::default()
    };
    assert_eq!(msm_bigint_with_config(&bases, &scalars, disabled), expected);
    scalars.fill(BigInt([0; 4]));
    for config in variants() {
        assert!(msm_bigint_with_config(&bases, &scalars, config).is_zero());
    }
}

#[test]
fn affine_and_glv_accept_noncanonical_full_width_integers() {
    let mut modulus_plus_one = Fq::MODULUS;
    modulus_plus_one.add_with_carry(&BigInt([1, 0, 0, 0]));
    let scalars = [
        Fq::MODULUS,
        modulus_plus_one,
        BigInt([u64::MAX; 4]),
        BigInt([0, 0, 0, 1 << 63]),
        BigInt([u64::MAX, u64::MAX, 0, 0]),
        (-Fq::ONE).into_bigint(),
        BigInt([0; 4]),
    ];
    let generator = Pallas::generator();
    let bases = [
        generator,
        -generator,
        generator,
        generator,
        generator,
        generator,
        Pallas::zero(),
    ];
    let expected = reference(&bases, &scalars);
    for glv in [false, true] {
        for width in [3, 8, 13] {
            let config = MsmSpikeConfig {
                window_bits: Some(width),
                filter_zero: true,
                trim_conversions: true,
                affine: true,
                glv,
            };
            assert_eq!(msm_bigint_with_config(&bases, &scalars, config), expected);
        }
    }
}

#[test]
fn cancellation_equation_and_changed_coefficient_are_distinguished() {
    let generator = Pallas::generator();
    let bases = [generator, generator, -generator, Pallas::zero()];
    let scalars =
        [Fq::from(5u64), Fq::from(7u64), Fq::from(12u64), Fq::ONE].map(PrimeField::into_bigint);
    for config in variants() {
        assert!(msm_bigint_with_config(&bases, &scalars, config).is_zero());
        let mut changed = scalars;
        changed[1] = Fq::from(8u64).into_bigint();
        assert_eq!(
            msm_bigint_with_config(&bases, &changed, config),
            generator.into_group()
        );
    }
}
