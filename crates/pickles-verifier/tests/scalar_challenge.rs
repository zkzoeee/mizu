//! Preserve the Halo bit-pair map while postponing field reduction.
use ark_ff::{AdditiveGroup, BigInt, Field, MontBackend, MontConfig, PrimeField};
use mina_curves::pasta::{Fp, Fq};
use mina_poseidon::sponge::{endo_coefficient, ScalarChallenge};

// Literal original field recurrence, independent of the integer implementation.
fn original<F: PrimeField>(value: F, bits: usize, endo: F) -> F {
    let repr = value.into_bigint();
    let words = repr.as_ref();
    let (mut a, mut b) = (F::from(2u64), F::from(2u64));
    for i in (0..bits / 2).rev() {
        a.double_in_place();
        b.double_in_place();
        let low = (words[(2 * i) / 64] >> ((2 * i) % 64)) & 1;
        let high = (words[(2 * i + 1) / 64] >> ((2 * i + 1) % 64)) & 1;
        let sign = if low == 0 { -F::ONE } else { F::ONE };
        if high == 0 {
            b += sign;
        } else {
            a += sign;
        }
    }
    a * endo + b
}

fn differential<F: PrimeField<BigInt = BigInt<4>>>() {
    let lambda = endo_coefficient::<F>();
    assert_ne!(lambda, F::ONE);
    assert_eq!(lambda.square() * lambda, F::ONE);
    let endos = [F::ZERO, F::ONE, -F::ONE, lambda, lambda.square()];
    let mut values = vec![
        F::ZERO,
        F::ONE,
        -F::ONE,
        F::from(u128::MAX),
        F::from(0xaaaa_aaaa_aaaa_aaaa_aaaa_aaaa_aaaa_aaaau128),
        F::from(0x5555_5555_5555_5555_5555_5555_5555_5555u128),
    ];
    for bit in 0..255 {
        let mut words = [0u64; 4];
        words[bit / 64] = 1 << (bit % 64);
        values.push(F::from_le_bytes_mod_order(
            &words
                .into_iter()
                .flat_map(u64::to_le_bytes)
                .collect::<Vec<_>>(),
        ));
    }
    let mut seed = 0x4596_f0aa_273c_b1d9u64;
    for _ in 0..512 {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_exact_mut(8) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            chunk.copy_from_slice(&seed.to_le_bytes());
        }
        values.push(F::from_le_bytes_mod_order(&bytes));
    }
    for value in values {
        for endo in endos {
            assert_eq!(
                ScalarChallenge::new(value).to_field_with_length(128, &endo),
                original(value, 128, endo)
            );
            assert_eq!(
                ScalarChallenge::new(value).to_field(&endo),
                original(value, 128, endo)
            );
        }
    }
}

#[test]
fn integer_128_matches_original_in_both_pasta_fields() {
    differential::<Fp>();
    differential::<Fq>();
}

fn fallback<F: PrimeField + std::panic::RefUnwindSafe>() {
    let endo = endo_coefficient::<F>();
    for value in [F::ZERO, F::ONE, -F::ONE, F::from(u128::MAX)] {
        for bits in [
            0, 1, 2, 3, 63, 64, 65, 126, 127, 128, 129, 254, 255, 256, 257,
        ] {
            assert_eq!(
                ScalarChallenge::new(value).to_field_with_length(bits, &endo),
                original(value, bits, endo)
            );
        }
    }
    // An oversized request must still panic rather than truncate.
    assert!(std::panic::catch_unwind(
        || ScalarChallenge::new(F::ONE).to_field_with_length(258, &endo)
    )
    .is_err());
    assert!(std::panic::catch_unwind(|| original(F::ONE, 258, endo)).is_err());
}
#[test]
fn other_lengths_keep_original_semantics() {
    fallback::<Fp>();
    fallback::<Fq>();
}

fn ignored_high<F: PrimeField>() {
    let endo = endo_coefficient::<F>();
    let low = F::from(0x81a7_947b_0bad_f00d_1234_5678_9abc_def0u128);
    let high = F::from(2u64).pow([128]) + F::from(2u64).pow([200]);
    assert_eq!(
        ScalarChallenge::new(low).to_field(&endo),
        ScalarChallenge::new(low + high).to_field(&endo)
    );
    let all_zero = ScalarChallenge::new(F::ZERO).to_field(&endo);
    assert_eq!(
        all_zero,
        F::from(1u128 << 65) * endo + F::from((1u128 << 64) + 1)
    );
    let all_one = ScalarChallenge::new(F::from(u128::MAX)).to_field(&endo);
    assert_eq!(
        all_one,
        F::from(3 * (1u128 << 64) - 1) * endo + F::from(1u128 << 65)
    );
}
#[test]
fn high_bits_and_integer_extremes_match_contract() {
    ignored_high::<Fp>();
    ignored_high::<Fq>();
}

#[derive(MontConfig)]
#[modulus = "17"]
#[generator = "3"]
struct SmallConfig;
type SmallField = ark_ff::Fp64<MontBackend<SmallConfig, 1>>;
#[test]
fn short_field_representation_keeps_fallback_and_bounds_failure() {
    let value = SmallField::from(7u64);
    for bits in [0, 1, 2, 32, 63, 64, 65] {
        assert_eq!(
            ScalarChallenge::new(value).to_field_with_length(bits, &SmallField::ONE),
            original(value, bits, SmallField::ONE)
        );
    }
    assert!(std::panic::catch_unwind(
        || ScalarChallenge::new(value).to_field_with_length(128, &SmallField::ONE)
    )
    .is_err());
}
