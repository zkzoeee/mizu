//! Challenge-polynomial coefficients kept in a closed canonical representation.
//! The order is the same reverse-challenge block expansion as b_poly_coefficients.
use alloc::vec::Vec;
use ark_ff::PrimeField;
use mina_curves::pasta::{
    canonical_words::{CanonicalPasta, CanonicalPastaRhs},
    Fp,
};

pub(crate) fn coefficients(challenges: &[Fp]) -> Vec<CanonicalPasta<false>> {
    assert!(
        challenges.len() < usize::BITS as usize,
        "challenge polynomial round count overflow"
    );
    let total = 1usize << challenges.len();
    let mut result = Vec::with_capacity(total);
    result.push(CanonicalPasta::from_limbs([1, 0, 0, 0]).expect("one is canonical"));
    for challenge in challenges.iter().rev() {
        let rhs = CanonicalPastaRhs::new(
            CanonicalPasta::from_limbs(challenge.into_bigint().0)
                .expect("field scalar is canonical"),
        );
        let len = result.len();
        debug_assert!(len * 2 <= result.capacity());
        // Expand directly into the reserved spare capacity, performing
        // `mul_prepared_assign` in place on the destination slot to avoid
        // per-element stack round-trips and `Vec::push` capacity branches.
        // SAFETY: `total == 1 << challenges.len()` was reserved up front, so
        // `len * 2 <= result.capacity()`, `result[..len]` is initialized, and
        // every slot in `len..2*len` is initialized before `set_len(len * 2)`.
        unsafe {
            let base = result.as_mut_ptr();
            for i in 0..len {
                let value = *base.add(i);
                let dst_ptr = base.add(len + i);
                dst_ptr.write(value);
                (*dst_ptr).mul_prepared_assign(&rhs);
            }
            result.set_len(len * 2);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::{AdditiveGroup, Field};
    use poly_commitment::commitment::b_poly_coefficients;

    #[test]
    fn canonical_coefficients_match_field_and_subset_product_oracles() {
        let mut seed = 0xc522_e186_99bf_4ac1u64;
        for rounds in (0..=12).chain([15, 16]) {
            let random: Vec<Fp> = (0..rounds)
                .map(|_| {
                    // Fill all limbs so the vectors exercise the full field range.
                    let bytes: Vec<u8> = (0..4)
                        .flat_map(|_| {
                            seed ^= seed << 13;
                            seed ^= seed >> 7;
                            seed ^= seed << 17;
                            seed.to_le_bytes()
                        })
                        .collect();
                    Fp::from_le_bytes_mod_order(&bytes)
                })
                .collect();
            for chals in [
                vec![Fp::ZERO; rounds],
                vec![Fp::ONE; rounds],
                vec![-Fp::ONE; rounds],
                random,
            ] {
                let actual = coefficients(&chals);
                let field = b_poly_coefficients(&chals);
                assert_eq!(actual.len(), field.len());
                for (i, (a, b)) in actual.iter().zip(&field).enumerate() {
                    assert_eq!(a.limbs(), b.into_bigint().0);
                    let subset = (0..rounds)
                        .filter(|j| i & (1 << j) != 0)
                        .fold(Fp::ONE, |p, j| p * chals[rounds - 1 - j]);
                    assert_eq!(a.limbs(), subset.into_bigint().0);
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "challenge polynomial round count overflow")]
    fn canonical_coefficients_reject_round_overflow_before_allocation() {
        coefficients(&vec![Fp::ONE; usize::BITS as usize]);
    }
}
