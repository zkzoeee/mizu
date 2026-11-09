//! Optional two-evaluation-point CIP schedules. The ordinary upstream function
//! remains the control. These preserve polynomial/chunk order and field values.
use alloc::vec::Vec;
use ark_ff::{Field, One, Zero};
use crate::types::StepField as F;

pub(crate) fn borrowed(xi: F, r: F, polys: &[Vec<Vec<F>>]) -> F {
    let mut result = F::zero();
    let mut weight = F::one();
    for pair in polys {
        assert_eq!(pair.len(), 2, "CIP requires two evaluation points");
        assert_eq!(pair[0].len(), pair[1].len(), "CIP chunk lengths differ");
        for (&a, &b) in pair[0].iter().zip(&pair[1]) {
            result += weight * (a + r * b);
            weight *= xi;
        }
    }
    result
}

pub(crate) fn fused(xi: F, r: F, polys: &[Vec<Vec<F>>]) -> F {
    // Accumulate the two evaluation-point sums independently. Four products
    // share one Montgomery correction, rather than correcting each product.
    let mut weights = [F::zero(); 4];
    let mut a = [F::zero(); 4];
    let mut b = [F::zero(); 4];
    let mut used = 0;
    let mut weight = F::one();
    let mut sums = [F::zero(); 2];
    for pair in polys {
        assert_eq!(pair.len(), 2, "CIP requires two evaluation points");
        assert_eq!(pair[0].len(), pair[1].len(), "CIP chunk lengths differ");
        for (&av, &bv) in pair[0].iter().zip(&pair[1]) {
            weights[used] = weight;
            a[used] = av;
            b[used] = bv;
            weight *= xi;
            used += 1;
            if used == 4 {
                sums[0] += F::sum_of_products(&weights, &a);
                sums[1] += F::sum_of_products(&weights, &b);
                used = 0;
            }
        }
    }
    if used != 0 {
        // Clear the previous full block's unused slots on the final block.
        weights[used..].fill(F::zero());
        sums[0] += F::sum_of_products(&weights, &a);
        sums[1] += F::sum_of_products(&weights, &b);
    }
    sums[0] + r * sums[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::PrimeField;
    use poly_commitment::commitment::combined_inner_product;

    #[test]
    fn schedules_match_upstream_over_chunk_and_block_boundaries() {
        let mut seed = 0x8ab3_0029_8471_36acu64;
        let mut random = || {
            let mut bytes = [0u8; 32];
            for chunk in bytes.chunks_mut(8) {
                seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
                chunk.copy_from_slice(&seed.to_le_bytes());
            }
            F::from_le_bytes_mod_order(&bytes)
        };
        for count in 0..18 {
            let polys: Vec<_> = (0..count).map(|i| {
                let n = i % 5;
                alloc::vec![(0..n).map(|_| random()).collect(),
                            (0..n).map(|_| random()).collect()]
            }).collect();
            for xi in [F::zero(), F::one(), -F::one(), random()] {
                for r in [F::zero(), F::one(), -F::one(), random()] {
                    let expected = combined_inner_product(&xi, &r, &polys);
                    assert_eq!(borrowed(xi, r, &polys), expected);
                    assert_eq!(fused(xi, r, &polys), expected);
                }
            }
        }
    }
}
