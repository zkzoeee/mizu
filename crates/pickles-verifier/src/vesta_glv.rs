//! Fixed-limb GLV decomposition for Vesta's `Fp` scalars.
//!
//! Adapted from Udon's integer Babai rounding and Vesta lattice parameters:
//! <https://github.com/zakura-core/common/blob/6de96c91f27533c52612b18d07f08e1d79337b49/crates/udon/src/curve/pasta/glv.rs>
//! <https://github.com/zakura-core/common/blob/6de96c91f27533c52612b18d07f08e1d79337b49/crates/udon/src/curve/pasta/parameters.rs>
//! The endomorphism orientation follows the same revision's
//! `crates/udon/src/field/pasta/parameters.rs`. This module changes neither
//! Mina's SRS nor its protocol endomorphism/challenge conventions: it only
//! supplies an equivalent decomposition of an already determined MSM scalar.
//!
//! Arithmetic uses fixed-size stack arrays, with no division or allocation.
//! It is variable-time, as is the verifier's existing public-input MSM.

// The MIT License (MIT)
//
// Copyright (c) The Zakura Contributors
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use ark_ff::{MontFp, PrimeField};
use mina_curves::pasta::{Fp, Fq};

// For n = Fp::MODULUS and lambda = 5^(2 * (n - 1) / 3), the kernel of
// (x, y) -> x + lambda*y mod n has columns (A, -B) and (B, D):
// A - lambda*B = B + lambda*D = 0 mod n; A*D + B*B = n.
const A: u128 = 0x49e69d1640f049157fcae1c700000000;
const B: u128 = 0x49e69d1640a899538cb1279300000001;
const D: u128 = A + B;

// The two lattice coefficients themselves, little-endian, two limbs each.
// No reciprocal of n is needed or computed: dividing by 2^254 instead of by n
// is what makes these exact, and the resulting bias is carried by the `delta`
// term in the bounds below.
const D_LIMBS: [u64; 2] = [0x0c7c095a00000001, 0x93cd3a2c8198e269];
const B_LIMBS: [u64; 2] = [0x8cb1279300000001, 0x49e69d1640a89953];

// With c1 = round(kD / 2^254), c2 = round(kB / 2^254), rounding errors
// eta1, eta2 of magnitude at most one half, and delta = n - 2^254:
//
//   k1 = k - A*c1 - B*c2 = -A*eta1 - B*eta2 - k*delta/2^254
//   k2 = B*c1 - D*c2     =  B*eta1 - D*eta2
//
// using A*D + B*B = n. Hence |k1| <= D/2 + k*delta/2^254, which is strictly
// below floor(D/2) + delta + 1, and |k2| <= (B + D)/2. The first bound is
// wider than the old reciprocal construction's by delta, because dividing by
// a power of two rather than by n leaves that bias behind; `decompose`'s
// halves really do exceed the old bound, so it is not a conservative
// restatement. Both stay below 2^127, and the larger one's top width-16 chunk
// is 0x6ed9, so eight width-16 digits still suffice -- see
// `CarryWindow::Elided` at the Vesta GLV call site.
const HALF_1_BOUND: u128 = 0x6c2d_3612_4a19_6a50_1f6b_359a_0000_0002;
const HALF_2_BOUND: u128 = (B + D) / 2 + 1;
/// The larger of the two half bounds, which is what the carry-window policy
/// has to clear.
const fn larger(a: u128, b: u128) -> u128 {
    if a > b {
        a
    } else {
        b
    }
}
pub(crate) const HALF_MAX: u128 = larger(HALF_1_BOUND, HALF_2_BOUND);

const _: () = {
    assert!(HALF_1_BOUND == (D >> 1) + 0x2246_98fc_094c_f91b_992d_30ed_0000_0001 + 1);
    assert!(HALF_1_BOUND < 1 << 127);
    assert!(HALF_2_BOUND < 1 << 127);
    // Eight width-16 digits need the top chunk plus a carry under 2^15.
    assert!((HALF_1_BOUND >> 112) + 1 < 1 << 15);
    assert!((HALF_2_BOUND >> 112) + 1 < 1 << 15);
};

/// Return beta for phi(x, y) = (beta*x, y), where phi(P) = [lambda]P.
///
/// Here beta = 5^((q - 1) / 3) in Fq, while lambda is the *squared*
/// canonical cube root in Fp. Tests check this pairing on Mina's generator.
#[inline]
pub(crate) fn endomorphism_coefficient() -> Fq {
    MontFp!("2942865608506852014473558576493638302197734138389222805617480874486368177743")
}

/// Split k into k1 + lambda*k2 mod n; each tuple is (negative, magnitude).
///
/// Both magnitudes are strictly below 2^127. Zero always has a false sign.
/// The input's field type establishes the canonical integer bound k < n.
#[inline]
pub(crate) fn decompose(scalar: Fp) -> ((bool, u128), (bool, u128)) {
    decompose_limbs(scalar.into_bigint().0)
}

/// The opaque residue constructor and arithmetic establish k < Fp::MODULUS.
#[inline]
pub(crate) fn decompose_canonical(scalar: mina_curves::pasta::canonical_words::CanonicalPasta<false>)
    -> ((bool, u128), (bool, u128))
{
    decompose_limbs(scalar.limbs())
}

#[inline]
fn decompose_limbs(limbs: [u64; 4]) -> ((bool, u128), (bool, u128)) {
    let c1 = rounded_quotient(&D_LIMBS, &limbs);
    let c2 = rounded_quotient(&B_LIMBS, &limbs);
    let half_1 = subtract_256(
        subtract_256(limbs, multiply_u128(c1, A)),
        multiply_u128(c2, B),
    );
    let half_2 = subtract_256(multiply_u128(c1, B), multiply_u128(c2, D));
    (signed_half(half_1), signed_half(half_2))
}

/// Multiply little-endian limb strings into an initially zeroed result.
fn schoolbook_multiply(lhs: &[u64], rhs: &[u64], result: &mut [u64]) {
    debug_assert_eq!(result.len(), lhs.len() + rhs.len());
    for (i, &x) in lhs.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &y) in rhs.iter().enumerate() {
            let sum = u128::from(result[i + j]) + u128::from(x) * u128::from(y) + u128::from(carry);
            result[i + j] = sum as u64;
            carry = (sum >> 64) as u64;
        }
        result[i + rhs.len()] = carry;
    }
}

/// round(coefficient * scalar / 2^254) for a two-limb coefficient.
///
/// Replaces the previous 5x4 product against a 2^384-scaled reciprocal of n.
/// Dividing by 2^254 needs no reciprocal at all, so the coefficient is the
/// lattice constant itself and the product is 2x4 rather than 5x4: eight
/// partial products instead of twenty. The cost of that is a wider bound on
/// the first half, carried by `delta` above.
///
/// A column-major (Comba) rewrite of the old 5x4 product was measured and
/// rejected at +11.0%, and writing it out with fixed array sizes measured
/// exactly zero -- LLVM already inlines and unrolls the slice-based helper.
/// Neither finding is affected by this change; see
/// spikes/artifacts/r24-comba-rejected/ and r33-decompose-profile/.
fn rounded_quotient(coefficient: &[u64; 2], scalar: &[u64; 4]) -> u128 {
    let mut product = [0u64; 6];
    schoolbook_multiply(coefficient, scalar, &mut product);
    // The quotient starts at bit 254, which is limb 3 bit 62, and bit 253
    // rounds it. `product[5] << 2` would drop quotient bits if limb 5 reached
    // 2^62; it cannot, because the product of a canonical scalar below 2^255
    // with a coefficient below 2^128 is below 2^382. At k = n - 1 limb 5 is
    // 0x24f34e8b2066389a for D and 0x1279a745902a2654 for B, both under 2^62.
    debug_assert!(product[5] < 1 << 62, "two-limb product exceeded 2^382");
    let low = (product[3] >> 62) | (product[4] << 2);
    let high = (product[4] >> 62) | (product[5] << 2);
    let round = u128::from((product[3] >> 61) & 1);
    let quotient = u128::from(low) | (u128::from(high) << 64);
    // The largest quotient is round((n-1)*D / 2^254), which is 128 bits with
    // room to spare below u128::MAX, so the rounding increment cannot wrap.
    debug_assert!(quotient < u128::MAX, "rounding increment would overflow");
    quotient + round
}

fn multiply_u128(lhs: u128, rhs: u128) -> [u64; 4] {
    let mut product = [0; 4];
    schoolbook_multiply(
        &[lhs as u64, (lhs >> 64) as u64],
        &[rhs as u64, (rhs >> 64) as u64],
        &mut product,
    );
    product
}

/// Subtraction modulo 2^256 preserves the final signed residual exactly.
fn subtract_256(lhs: [u64; 4], rhs: [u64; 4]) -> [u64; 4] {
    let mut result = [0; 4];
    let mut borrow = false;
    for i in 0..4 {
        let (difference, first_borrow) = lhs[i].overflowing_sub(rhs[i]);
        let (difference, second_borrow) = difference.overflowing_sub(u64::from(borrow));
        result[i] = difference;
        borrow = first_borrow || second_borrow;
    }
    result
}

/// Decode the proven signed-127-bit residual from its 256-bit two's complement.
fn signed_half(value: [u64; 4]) -> (bool, u128) {
    let sign_extension = if value[1] >> 63 == 0 { 0 } else { u64::MAX };
    debug_assert_eq!(value[2], sign_extension);
    debug_assert_eq!(value[3], sign_extension);
    let low = u128::from(value[0]) | (u128::from(value[1]) << 64);
    let signed = low as i128;
    debug_assert_ne!(signed, i128::MIN);
    (signed < 0, signed.unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ec::{AffineRepr, CurveGroup};
    use ark_ff::{AdditiveGroup, BigInt, BigInteger, Field, Zero};
    use mina_curves::pasta::Vesta;
    use rand_chacha::ChaCha20Rng;
    use rand_core::{RngCore, SeedableRng};

    const LAMBDA: Fp =
        MontFp!("8503465768106391777493614032514048814691664078728891710322960303815233784505");

    fn signed_field((negative, magnitude): (bool, u128)) -> Fp {
        let value = Fp::from(magnitude);
        if negative {
            -value
        } else {
            value
        }
    }

    fn check_decomposition(scalar: Fp) {
        let (a, b) = decompose(scalar);
        assert!(a.1 < HALF_1_BOUND);
        assert!(b.1 < HALF_2_BOUND);
        assert!(a.1 < 1 << 127 && b.1 < 1 << 127);
        assert!(!a.0 || a.1 != 0, "no negative zero");
        assert!(!b.0 || b.1 != 0, "no negative zero");
        assert_eq!(signed_field(a) + LAMBDA * signed_field(b), scalar);
    }

    #[test]
    fn lattice_and_endomorphism_constants_match_mina_vesta() {
        assert_eq!(Fp::from(A) - LAMBDA * Fp::from(B), Fp::ZERO);
        assert_eq!(Fp::from(B) + LAMBDA * Fp::from(D), Fp::ZERO);
        let mut determinant = BigInt(multiply_u128(A, D));
        assert!(!determinant.add_with_carry(&BigInt(multiply_u128(B, B))));
        assert_eq!(determinant, Fp::MODULUS);
        assert_ne!(LAMBDA, Fp::ONE);
        assert_eq!(LAMBDA.square() + LAMBDA + Fp::ONE, Fp::ZERO);
        let beta = endomorphism_coefficient();
        assert_ne!(beta, Fq::ONE);
        assert_eq!(beta.square() + beta + Fq::ONE, Fq::ZERO);
        let generator = Vesta::generator();
        let mapped = Vesta::new_unchecked(generator.x * beta, generator.y);
        assert!(mapped.is_on_curve());
        assert_eq!(mapped, (generator * LAMBDA).into_affine());
    }

    #[test]
    fn known_integer_decompositions_match() {
        assert_eq!(decompose(Fp::ZERO), ((false, 0), (false, 0)));
        assert_eq!(decompose(Fp::ONE), ((false, 1), (false, 0)));
        assert_eq!(decompose(-Fp::ONE), ((true, 1), (false, 0)));
        assert_eq!(
            decompose(Fp::from(u128::MAX)),
            (
                (false, 45589192707464890216754169281618378750),
                (true, 86663725065984043395317759),
            )
        );
        assert_eq!(
            decompose(Fp::from(2u64).pow([254])),
            ((true, 45560315531419706090280762371685220353), (false, 0))
        );
    }

    #[test]
    fn boundaries_and_random_full_width_scalars_reconstruct_within_bounds() {
        for scalar in [
            Fp::ZERO,
            Fp::ONE,
            -Fp::ONE,
            Fp::from(A),
            Fp::from(B),
            Fp::from(D),
            Fp::from(u128::MAX),
            LAMBDA,
            -LAMBDA,
        ] {
            check_decomposition(scalar);
        }
        let mut power = Fp::ONE;
        for _ in 0..255 {
            for scalar in [power - Fp::ONE, power, power + Fp::ONE, -power] {
                check_decomposition(scalar);
            }
            power.double_in_place();
        }
        let mut rng = ChaCha20Rng::from_seed([0x47; 32]);
        for _ in 0..2048 {
            let mut bytes = [0; 32];
            rng.fill_bytes(&mut bytes);
            check_decomposition(Fp::from_le_bytes_mod_order(&bytes));
        }
    }

    #[test]
    fn decomposed_point_multiplication_matches_reference() {
        let generator = Vesta::generator();
        let beta = endomorphism_coefficient();
        let mut rng = ChaCha20Rng::from_seed([0x56; 32]);
        for i in 0..36 {
            let mut bytes = [0; 32];
            rng.fill_bytes(&mut bytes);
            let scalar = match i {
                0 => Fp::ZERO,
                1 => Fp::ONE,
                2 => -Fp::ONE,
                3 => Fp::from(2u64).pow([254]),
                _ => Fp::from_le_bytes_mod_order(&bytes),
            };
            let base = match i % 5 {
                0 => Vesta::zero(),
                1 => generator,
                2 => -generator,
                _ => (generator * Fp::from(i as u64 + 1)).into_affine(),
            };
            let mapped = Vesta {
                x: base.x * beta,
                y: base.y,
                infinity: base.infinity,
            };
            let (a, b) = decompose(scalar);
            let mut left = base.mul_bigint([a.1 as u64, (a.1 >> 64) as u64]);
            let mut right = mapped.mul_bigint([b.1 as u64, (b.1 >> 64) as u64]);
            if a.0 {
                left = -left;
            }
            if b.0 {
                right = -right;
            }
            assert_eq!(left + right, base * scalar, "case {i}");
        }
    }
}

#[cfg(test)]
mod quotient_tests {
    use super::{decompose, rounded_quotient, B_LIMBS, D_LIMBS, HALF_1_BOUND, HALF_2_BOUND};
    use ark_ff::{BigInt, PrimeField};
    use mina_curves::pasta::Fp;

    /// Fp::MODULUS, little-endian. Cross-checked against A*D + B*B.
    const N: [u64; 4] = [
        0x992d30ed00000001,
        0x224698fc094cf91b,
        0x0000000000000000,
        0x4000000000000000,
    ];

    /// Independent oracle in base 2^32: a different radix, a different carry
    /// structure, and no `u128` multiplication anywhere, so it shares no
    /// arithmetic path with the implementation. Carried over from the
    /// rejected-Comba round and re-aimed at the two-limb quotient.
    fn quotient_base32_oracle(coefficient: &[u64; 2], scalar: &[u64; 4]) -> u128 {
        let split = |limbs: &[u64]| -> Vec<u64> {
            let mut out = Vec::with_capacity(limbs.len() * 2);
            for &l in limbs {
                out.push(l & 0xffff_ffff);
                out.push(l >> 32);
            }
            out
        };
        let a = split(coefficient); // 4 half-limbs
        let b = split(scalar); //     8 half-limbs
        let mut p = vec![0u64; a.len() + b.len()];
        for (i, &x) in a.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &y) in b.iter().enumerate() {
                let s = p[i + j] + x * y + carry; // every term < 2^64
                p[i + j] = s & 0xffff_ffff;
                carry = s >> 32;
            }
            let mut k = i + b.len();
            while carry != 0 {
                let s = p[k] + carry;
                p[k] = s & 0xffff_ffff;
                carry = s >> 32;
                k += 1;
            }
        }
        let limb = |idx: usize| -> u64 { p[2 * idx] | (p[2 * idx + 1] << 32) };
        assert!(limb(5) < 1 << 62, "oracle: limb 5 must stay below 2^62");
        let low = (limb(3) >> 62) | (limb(4) << 2);
        let high = (limb(4) >> 62) | (limb(5) << 2);
        let round = u128::from((limb(3) >> 61) & 1);
        (u128::from(low) | (u128::from(high) << 64)) + round
    }

    fn less_than_n(v: &[u64; 4]) -> bool {
        for i in (0..4).rev() {
            if v[i] != N[i] {
                return v[i] < N[i];
            }
        }
        false
    }

    /// Scalars placing the product's fractional part exactly on 2^253 and one
    /// unit either side of it, so the rounding decision is on the point of
    /// flipping and every low column matters. Derived offline from
    /// `k = (2^253 + e) * c^-1 mod 2^254` with exact big integers, then pinned.
    /// `k = 2^253` is the exact tie for any odd coefficient, which both share.
    const TIE: [u64; 4] = [0, 0, 0, 0x2000_0000_0000_0000];
    const D_BELOW: [u64; 4] = [
        0x0c7c0959ffffffff,
        0xd22e350702116ec5,
        0x94b0bd21c0a9fd28,
        0x029250be43b7b677,
    ];
    const D_ABOVE: [u64; 4] = [
        0xf383f6a600000001,
        0x2dd1caf8fdee913a,
        0x6b4f42de3f5602d7,
        0x3d6daf41bc484988,
    ];
    const B_BELOW: [u64; 4] = [
        0x8cb12792ffffffff,
        0x012f752eb9447aea,
        0x0021da2802f2d877,
        0x078e10f056cc2359,
    ];
    const B_ABOVE: [u64; 4] = [
        0x734ed86d00000001,
        0xfed08ad146bb8515,
        0xffde25d7fd0d2788,
        0x3871ef0fa933dca6,
    ];

    fn vectors() -> Vec<[u64; 4]> {
        let mut out: Vec<[u64; 4]> = Vec::new();
        out.push([0; 4]);
        out.push([1, 0, 0, 0]);
        out.push([N[0] - 1, N[1], N[2], N[3]]); // n - 1, the worst case
        out.push([u64::MAX, u64::MAX, u64::MAX, 0]);
        out.push([u64::MAX, u64::MAX, 0, 0]);
        out.extend_from_slice(&[TIE, D_BELOW, D_ABOVE, B_BELOW, B_ABOVE]);
        let mut state = 0x243f_6a88_85a3_08d3u64;
        while out.len() < 600 {
            let mut v = [0u64; 4];
            for slot in v.iter_mut() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *slot = state;
            }
            v[3] &= 0x3fff_ffff_ffff_ffff;
            if less_than_n(&v) {
                out.push(v);
            }
        }
        out
    }

    #[test]
    fn quotient_matches_an_independent_oracle() {
        let mut checked = 0usize;
        for scalar in vectors() {
            for coefficient in [&D_LIMBS, &B_LIMBS] {
                assert_eq!(
                    rounded_quotient(coefficient, &scalar),
                    quotient_base32_oracle(coefficient, &scalar),
                    "implementation vs base-2^32 oracle on {scalar:016x?}"
                );
                checked += 1;
            }
        }
        assert!(checked >= 1200, "expected both coefficients over every vector");
    }

    /// Guards the boundary vectors themselves: below the tie the rounding bit
    /// must be clear and at or above it set. If a coefficient ever changes
    /// these stop being boundary cases and the oracle test silently weakens.
    #[test]
    fn rounding_boundary_vectors_really_straddle_the_tie() {
        for (coefficient, below, above) in [
            (&D_LIMBS, &D_BELOW, &D_ABOVE),
            (&B_LIMBS, &B_BELOW, &B_ABOVE),
        ] {
            let round_bit = |scalar: &[u64; 4]| -> u64 {
                let mut product = [0u64; 6];
                super::schoolbook_multiply(coefficient, scalar, &mut product);
                (product[3] >> 61) & 1
            };
            assert_eq!(round_bit(below), 0, "below the tie should round down");
            assert_eq!(round_bit(&TIE), 1, "the exact tie should round up");
            assert_eq!(round_bit(above), 1, "above the tie should round up");
            // And the implementation's answer must move with that decision.
            let q_below = rounded_quotient(coefficient, below);
            let q_tie = rounded_quotient(coefficient, &TIE);
            assert_eq!(q_below, quotient_base32_oracle(coefficient, below));
            assert_eq!(q_tie, quotient_base32_oracle(coefficient, &TIE));
        }
    }

    /// The top-limb bound, at the only place it can be tight. Known answers
    /// computed offline with exact integers, not read back from this code.
    #[test]
    fn top_limb_stays_below_two_to_the_62_at_the_modulus_edge() {
        let n_minus_one = [N[0] - 1, N[1], N[2], N[3]];
        for (coefficient, expected) in [
            (&D_LIMBS, 0x24f3_4e8b_2066_389au64),
            (&B_LIMBS, 0x1279_a745_902a_2654u64),
        ] {
            let mut product = [0u64; 6];
            super::schoolbook_multiply(coefficient, &n_minus_one, &mut product);
            assert_eq!(product[5], expected, "limb 5 at n - 1");
            assert!(product[5] < 1 << 62, "limb 5 must stay below 2^62");
            // The quotient itself must not be able to wrap on the rounding
            // increment.
            let q = rounded_quotient(coefficient, &n_minus_one);
            assert!(q < u128::MAX, "quotient left no room to round");
        }
    }

    #[test]
    fn decompose_halves_stay_inside_their_bounds() {
        let mut widest_1 = 0u128;
        let mut widest_2 = 0u128;
        for scalar in vectors() {
            let field = Fp::from_bigint(BigInt(scalar)).expect("canonical");
            let ((_, k1), (_, k2)) = decompose(field);
            assert!(k1 < HALF_1_BOUND, "k1 {k1:#x} exceeds its bound");
            assert!(k2 < HALF_2_BOUND, "k2 {k2:#x} exceeds its bound");
            widest_1 = widest_1.max(k1);
            widest_2 = widest_2.max(k2);
        }
        // The first bound really is needed in its widened form: the halves
        // exceed the old reciprocal construction's bound of
        // 0x49e69d1640cc7134863e04ad00000001 on these very vectors, so this is
        // not a conservative restatement of it.
        assert!(
            widest_1 > 0x49e6_9d16_40cc_7134_863e_04ad_0000_0001,
            "widest k1 {widest_1:#x} never exceeded the old bound, so the \
             widened bound is untested here"
        );
        assert!(widest_1 < 1 << 127 && widest_2 < 1 << 127);
    }
}
