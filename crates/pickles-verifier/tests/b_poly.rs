//! Compare coefficient order against the closed-form subset product.
use ark_ff::{AdditiveGroup, Field, PrimeField};
use mina_curves::pasta::{Fp, Fq};
use poly_commitment::commitment::{b_poly, b_poly_coefficients, b_poly_coefficients_into};
fn cases<F: PrimeField>() {
    let mut seed = 0x175f_b4aa_6a80_2fe1u64;
    for rounds in (0..=12).chain([15, 16]) {
        let random: Vec<F> = (0..rounds)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                F::from(seed)
            })
            .collect();
        for chals in [
            vec![F::ZERO; rounds],
            vec![F::ONE; rounds],
            vec![-F::ONE; rounds],
            random,
        ] {
            let expected: Vec<F> = (0..1usize << rounds)
                .map(|i| {
                    (0..rounds)
                        .filter(|j| i & (1 << j) != 0)
                        .fold(F::ONE, |p, j| p * chals[rounds - 1 - j])
                })
                .collect();
            let actual = b_poly_coefficients(&chals);
            assert_eq!(actual, expected);
            let mut into = vec![F::from(123u64); expected.len()];
            b_poly_coefficients_into(&chals, &mut into);
            assert_eq!(
                into, expected,
                "every destination entry must be overwritten"
            );
            for x in [F::ZERO, F::ONE, -F::ONE, F::from(7u64)] {
                let eval = expected.iter().rev().fold(F::ZERO, |a, c| a * x + c);
                assert_eq!(b_poly(&chals, x), eval);
            }
        }
    }
}
#[test]
fn both_fields_preserve_coefficient_order_and_polynomial_evaluations() {
    cases::<Fp>();
    cases::<Fq>();
}

fn mixed_prefixes<F: PrimeField>() {
    for active_rounds in 0..=8 {
        for first_rounds in 0..=active_rounds {
            let chals: Vec<F> = (0..first_rounds)
                .map(|i| F::from((3 * i + 5) as u64))
                .collect();
            let active = 1usize << active_rounds;
            let count = 1usize << first_rounds;
            let mut expected = vec![F::ZERO; active + 4];
            expected[0] = F::from(91u64);
            expected[active + 1..].fill(F::from(92u64));
            let mut actual = expected.clone();
            expected[1..1 + count].copy_from_slice(&b_poly_coefficients(&chals));
            b_poly_coefficients_into(&chals, &mut actual[1..1 + count]);
            assert_eq!(
                actual, expected,
                "h, longer SRS suffix, and proof terms must be preserved"
            );
        }
    }
}
#[test]
fn direct_first_proof_preserves_mixed_round_suffix_and_other_terms() {
    mixed_prefixes::<Fp>();
    mixed_prefixes::<Fq>();
}
#[test]
#[should_panic(expected = "challenge polynomial buffer length")]
fn direct_coefficients_reject_wrong_buffer_length() {
    b_poly_coefficients_into(&[Fp::ONE], &mut [Fp::ZERO; 3]);
}
#[test]
#[should_panic(expected = "challenge polynomial round count overflow")]
fn direct_coefficients_reject_round_overflow_before_length_check() {
    b_poly_coefficients_into(&vec![Fp::ONE; usize::BITS as usize], &mut []);
}
