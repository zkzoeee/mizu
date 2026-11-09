//! Isolated second-round MSM experiments. Protocol scalars and points are unchanged.
use alloc::{vec, vec::Vec};
use ark_ec::{
    short_weierstrass::{Affine, Projective, SWCurveConfig},
    AffineRepr,
};
use ark_ff::{AdditiveGroup, BigInt, BigInteger, PrimeField, Zero};
use mina_curves::pasta::Fp;

pub(crate) fn window(size: usize) -> usize {
    match option_env!("ZEKO_VESTA_WINDOW") {
        Some(value) => {
            let width = value
                .parse::<usize>()
                .expect("ZEKO_VESTA_WINDOW must be 2..16");
            assert!((2..=16).contains(&width), "ZEKO_VESTA_WINDOW must be 2..16");
            width
        }
        None => {
            if size < 32 {
                3
            } else {
                // Match ark_std's ceil(log2(size)) heuristic without floating point.
                ((usize::BITS - (size - 1).leading_zeros()) as usize * 69 / 100) + 2
            }
        }
    }
}

pub(crate) fn run<C: SWCurveConfig<ScalarField = Fp>>(
    bases: &[Affine<C>],
    scalars: &[BigInt<4>],
    bits: usize,
    // The caller's exclusive magnitude bound, when it has one. `None` keeps
    // the carry window unconditionally. The policy is decided here because
    // this is where the width is known.
    bound: Option<u128>,
) -> Projective<C>
where
    C::BaseField: PrimeField<BigInt = BigInt<4>>,
{
    let width = window(bases.len().min(scalars.len()));
    let carry = bound.map_or(
        mina_curves::pasta::point_spikes::CarryWindow::Reserved,
        |bound| {
            mina_curves::pasta::point_spikes::CarryWindow::for_bound(bound, width, bits)
        },
    );
    if matches!(option_env!("ZEKO_VESTA_POINT_PRECOMPILE"), Some("1")) {
        mina_curves::pasta::point_spikes::msm::<C>(
            bases,
            scalars,
            width,
            bits,
            carry,
            mina_curves::pasta::point_spikes::PastaCurve::Vesta,
        )
    } else if matches!(option_env!("ZEKO_VESTA_AFFINE"), Some("1")) {
        ark_ec::scalar_mul::variable_base::affine_spike::msm_affine::<C>(
            bases, scalars, width, bits,
        )
    } else {
        projective_msm::<C>(bases, scalars, width, bits)
    }
}

/// Signed-window projective control for shorter GLV/prepared scalar streams.
/// Retains an extra carry window, including when `bits` is a multiple of width.
pub(crate) fn projective_msm<C: SWCurveConfig<ScalarField = Fp>>(
    bases: &[Affine<C>],
    scalars: &[BigInt<4>],
    width: usize,
    bits: usize,
) -> Projective<C> {
    assert!((2..=16).contains(&width) && (1..=255).contains(&bits));
    let n = bases.len().min(scalars.len());
    let windows = bits.div_ceil(width) + 1;
    let mut digits = Vec::with_capacity(n * windows);
    let mut top_window = 0;
    for scalar in &scalars[..n] {
        assert!(
            scalar.num_bits() as usize <= bits,
            "nonzero truncated MSM bits"
        );
        let mut carry = 0i32;
        for j in 0..windows {
            let offset = j * width;
            let limb = offset / 64;
            let shift = offset % 64;
            let mut word = scalar.0.get(limb).copied().unwrap_or(0) >> shift;
            if shift != 0 {
                word |= scalar.0.get(limb + 1).copied().unwrap_or(0) << (64 - shift);
            }
            let digit = carry + (word & ((1 << width) - 1)) as i32;
            carry = (digit + (1 << (width - 1))) >> width;
            let signed = digit - (carry << width);
            if signed != 0 {
                top_window = top_window.max(j);
            }
            digits.push(signed);
        }
        assert_eq!(carry, 0);
    }
    let mut result = Projective::<C>::zero();
    let mut buckets = vec![Projective::<C>::zero(); 1 << (width - 1)];
    for j in (0..=top_window).rev() {
        if j != top_window {
            for _ in 0..width {
                result.double_in_place();
            }
        }
        buckets.fill(Projective::<C>::zero());
        for (base, row) in bases[..n].iter().zip(digits.chunks(windows)) {
            let digit = row[j];
            if digit > 0 {
                buckets[digit as usize - 1] += base;
            } else if digit < 0 {
                buckets[-digit as usize - 1] -= base;
            }
        }
        let mut running = Projective::<C>::zero();
        for bucket in buckets.iter().rev() {
            running += bucket;
            result += running;
        }
    }
    result
}

/// Expand each scalar by GLV straight into the MSM's own term buffers.
///
/// The materialising path writes every expanded point into a `Vec<Affine>`
/// that the MSM reads back and converts; this converts each point while it is
/// still in hand. Unlike the rejected streaming shape it knows its own term
/// count once the loop ends, so `window` keeps its size-dependent heuristic
/// and no knob restriction is needed.
pub(crate) fn run_glv_prepared<C: SWCurveConfig<ScalarField = Fp>>(
    bases: &[Affine<C>],
    scalars: &[Fp],
    beta: C::BaseField,
) -> Projective<C>
where
    C::BaseField: PrimeField<BigInt = BigInt<4>>,
{
    run_glv_prepared_with_conversion(bases, scalars, beta, |base| *base)
}

/// Materialize final terms while converting each original base exactly once.
/// The original base slice retains its provenance; the converter must preserve
/// its point. No intermediate canonical-base vector or streaming MSM is used.
pub(crate) fn run_glv_prepared_with_conversion<C, B, Convert>(
    bases: &[B], scalars: &[Fp], beta: C::BaseField, convert: Convert,
) -> Projective<C>
where
    C: SWCurveConfig<ScalarField = Fp>,
    C::BaseField: PrimeField<BigInt = BigInt<4>>,
    Convert: Fn(&B) -> Affine<C>,
{
    run_glv_prepared_with_decomposition(bases, scalars, beta, convert, |s| crate::vesta_glv::decompose(*s))
}

pub(crate) fn run_glv_prepared_canonical_with_conversion<C, B, Convert>(
    bases: &[B], scalars: &[mina_curves::pasta::canonical_words::CanonicalPasta<false>],
    beta: C::BaseField, convert: Convert,
) -> Projective<C>
where
    C: SWCurveConfig<ScalarField = Fp>,
    C::BaseField: PrimeField<BigInt = BigInt<4>>,
    Convert: Fn(&B) -> Affine<C>,
{
    run_glv_prepared_with_decomposition(bases, scalars, beta, convert, |s| crate::vesta_glv::decompose_canonical(*s))
}

fn run_glv_prepared_with_decomposition<C, B, S, Convert, Decompose>(
    bases: &[B], scalars: &[S], beta: C::BaseField, convert: Convert, decompose: Decompose,
) -> Projective<C>
where
    C: SWCurveConfig<ScalarField = Fp>,
    C::BaseField: PrimeField<BigInt = BigInt<4>>,
    Convert: Fn(&B) -> Affine<C>,
    Decompose: Fn(&S) -> ((bool, u128), (bool, u128)),
{
    use mina_curves::pasta::point_spikes::{msm_prepared, PastaCurve, PreparedTerms};
    assert_eq!(bases.len(), scalars.len());
    let mut terms = PreparedTerms::<C>::with_capacity(2 * bases.len(), PastaCurve::Vesta);
    for (base, scalar) in bases.iter().zip(scalars) {
        let base = convert(base);
        let ((neg1, k1), (neg2, k2)) = decompose(scalar);
        for (negative, k, endo) in [(neg1, k1, false), (neg2, k2, true)] {
            if k == 0 || base.is_zero() {
                continue;
            }
            let mut point = base;
            if endo {
                point.x *= beta;
            }
            if negative {
                point.y = -point.y;
            }
            terms.push(&point, BigInt([k as u64, (k >> 64) as u64, 0, 0]));
        }
    }
    let width = window(terms.len());
    // The width is only known now, so the policy is decided now, from the
    // decomposition's proven bound rather than from an assumption about which
    // width this path uses.
    let carry = mina_curves::pasta::point_spikes::CarryWindow::for_bound(
        crate::vesta_glv::HALF_MAX,
        width,
        128,
    );
    msm_prepared(&terms, width, 128, carry)
}

pub(crate) fn expand_glv<C: SWCurveConfig<ScalarField = Fp>>(
    bases: &[Affine<C>],
    scalars: &[Fp],
    beta: C::BaseField,
) -> (Vec<Affine<C>>, Vec<BigInt<4>>) {
    assert_eq!(bases.len(), scalars.len());
    let mut points = Vec::with_capacity(2 * bases.len());
    let mut coefficients = Vec::with_capacity(2 * bases.len());
    for (base, scalar) in bases.iter().zip(scalars) {
        let ((neg1, k1), (neg2, k2)) = crate::vesta_glv::decompose(*scalar);
        for (negative, k, endo) in [(neg1, k1, false), (neg2, k2, true)] {
            if k == 0 || base.is_zero() {
                continue;
            }
            let mut point = *base;
            if endo {
                point.x *= beta;
            }
            if negative {
                // In place: `-point` for an Affine returns a fresh value, so
                // `point = -point` copied the whole struct to flip one
                // coordinate. The zero case is already excluded above.
                point.y = -point.y;
            }
            points.push(point);
            coefficients.push(BigInt([k as u64, (k >> 64) as u64, 0, 0]));
        }
    }
    (points, coefficients)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ec::CurveGroup;
    use mina_curves::pasta::{Vesta, VestaParameters};
    /// The prepared-builder GLV path must agree with expanding into vectors
    /// and calling the MSM. Covers the identity base and the zero scalar, both
    /// of which drop terms, so the two paths see different term counts than
    /// their capacities.
    #[test]
    fn prepared_glv_matches_the_expanded_path() {
        use mina_curves::pasta::point_spikes::{msm, CarryWindow, PastaCurve};
        let g = Vesta::generator();
        let bases: Vec<_> = (0..13)
            .map(|i| match i % 4 {
                0 => Vesta::zero(),
                1 => g,
                2 => -g,
                _ => (g * Fp::from(i + 5)).into_affine(),
            })
            .collect();
        let scalars: Vec<_> = (0..bases.len())
            .map(|i| if i % 5 == 0 { Fp::ZERO } else { Fp::from(i as u64 * 7 + 1) })
            .collect();
        let beta = crate::vesta_glv::endomorphism_coefficient();
        let (points, halves) = expand_glv::<VestaParameters>(&bases, &scalars, beta);
        assert!(points.len() < 2 * bases.len(), "terms must be dropped here");
        let width = window(points.len());
        let expanded = msm::<VestaParameters>(
            &points,
            &halves,
            width,
            128,
            CarryWindow::for_bound(crate::vesta_glv::HALF_MAX, width, 128),
            PastaCurve::Vesta,
        );
        let prepared = run_glv_prepared::<VestaParameters>(&bases, &scalars, beta);
        assert_eq!(prepared.into_affine(), expanded.into_affine());
        // A differently shaped original representation is converted once per
        // base, including identities and zero-scaled bases, then enters the
        // same final term builder. Compare to an independent scalar-mul sum.
        let originals: Vec<_> = bases.iter().copied().zip(0usize..).collect();
        let calls = core::cell::Cell::new(0usize);
        let converted = run_glv_prepared_with_conversion::<VestaParameters, _, _>(
            &originals, &scalars, beta, |entry| {
                calls.set(calls.get() + 1);
                entry.0
            },
        );
        let reference = bases.iter().zip(&scalars).fold(
            Projective::<VestaParameters>::zero(), |sum, (point, scalar)| sum + *point * scalar,
        );
        assert_eq!(calls.get(), bases.len());
        assert_eq!(converted, reference);
    }

    /// `Shifts::shifts_only` must agree with `Shifts::new().shifts()` at every
    /// domain size this verifier can see. The only difference between them is
    /// the cell map, which nothing on the deferred-values path reads, but the
    /// sampling must be bit-identical or the permutation argument changes.
    #[test]
    fn shifts_only_matches_the_full_constructor() {
        use ark_ff::Field;
        use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
        use kimchi::circuits::polynomials::permutation::Shifts;
        for log2 in 1..=12usize {
            let size = 1usize << log2;
            let domain = Radix2EvaluationDomain::<Fp>::new(size)
                .unwrap_or_else(|| panic!("no radix-2 domain of size {size}"));
            let full = *Shifts::<Fp>::new(&domain).shifts();
            let only = Shifts::<Fp>::shifts_only(&domain);
            assert_eq!(only, full, "shifts disagree at 2^{log2}");
            // The documented properties of the sampling, checked independently
            // of either constructor: first shift is one, all seven distinct,
            // and none lies in the domain's vanishing set.
            assert_eq!(only[0], Fp::ONE);
            // The identity shift is deliberately inside the domain -- it is
            // set directly, not sampled. Only the sampled shifts are required
            // to lie outside it.
            assert!(domain.evaluate_vanishing_polynomial(only[0]).is_zero());
            for i in 0..only.len() {
                for j in (i + 1)..only.len() {
                    assert_ne!(only[i], only[j], "shifts {i} and {j} collide at 2^{log2}");
                }
                if i > 0 {
                    assert!(
                        !domain.evaluate_vanishing_polynomial(only[i]).is_zero(),
                        "sampled shift {i} is a domain element at 2^{log2}"
                    );
                    assert!(
                        only[i].legendre().is_qnr(),
                        "sampled shift {i} is not a non-residue at 2^{log2}"
                    );
                }
            }
        }
    }

    /// The carry policy must be decided from the width actually in use, not
    /// asserted for the one a caller happens to run — and it must be *tight*,
    /// not merely cautious. So this pins an equivalence rather than a table:
    /// for every width, `for_bound` elides exactly when recoding the worst
    /// permitted magnitude into the elided window count leaves no carry.
    ///
    /// That makes the test independent of the bounds. When r58 raised the
    /// Pallas bound from 0.866 to 0.999915 of 2^127, widths 4 and 8 stopped
    /// eliding — and the equivalence shows that is necessary rather than a
    /// regression in the policy: at width 4 the top chunk is 7 against a
    /// signed digit range of [-8, 7], so representing 7 + 1 carries out, and
    /// at width 8 it is 127 against [-128, 127]. Reaching them needs a
    /// `report` of which widths reserve, below.
    #[test]
    fn carry_policy_elides_exactly_when_the_worst_magnitude_does_not_carry() {
        use mina_curves::pasta::point_spikes::CarryWindow;
        let bounds = [
            ("vesta", crate::vesta_glv::HALF_MAX),
            ("pallas", mina_curves::pasta::pallas_glv::HALF_MAX),
        ];
        for (name, bound) in bounds {
            assert!(bound < 1 << 127, "{name} bound must stay under 2^127");
            let mut reserved = alloc::vec![];
            for width in 2..=16usize {
                let windows = 128usize.div_ceil(width);
                let (_, carry) = recode_u128(bound - 1, width, windows);
                let policy = CarryWindow::for_bound(bound, width, 128);
                let expected = if carry == 0 {
                    CarryWindow::Elided
                } else {
                    CarryWindow::Reserved
                };
                assert_eq!(
                    policy, expected,
                    "{name} width {width}: policy and the worst-magnitude recoding disagree"
                );
                if policy == CarryWindow::Reserved {
                    reserved.push(width);
                }
            }
            // Width 2 reserves for every bound here: the top window sits at
            // bit 126, every bound presents a chunk of 1, and 1 + 1 is not
            // below 2.
            assert!(reserved.contains(&2), "{name} must reserve at width 2");
            match name {
                "vesta" => assert_eq!(reserved, alloc::vec![2]),
                "pallas" => assert_eq!(reserved, alloc::vec![2, 4, 8]),
                _ => unreachable!(),
            }
        }
    }

    /// The widths the Pallas MSM can actually select, against the ones that
    /// reserve. `adaptive_window` returns 3 below 32 terms and
    /// `bits*69/100 + 2` at or above it, where `bits` is at least 6 — so width
    /// 4 is unreachable and cannot cost anything, while width 8 is reachable
    /// for 257 to 1023 expanded terms and pays one extra window there.
    #[test]
    fn width_four_is_unreachable_and_width_eight_is_not() {
        fn adaptive(size: usize) -> usize {
            if size < 32 {
                3
            } else {
                ((usize::BITS - (size - 1).leading_zeros()) as usize * 69 / 100) + 2
            }
        }
        let mut seen = alloc::vec![];
        for size in 1..4096usize {
            let width = adaptive(size);
            if !seen.contains(&width) {
                seen.push(width);
            }
            assert_ne!(width, 4, "width 4 reachable at {size} terms");
        }
        assert!(seen.contains(&8), "width 8 should be reachable");
        assert_eq!(adaptive(256), 7);
        assert_eq!(adaptive(257), 8);
        assert_eq!(adaptive(1024), 8);
        assert_eq!(adaptive(1025), 9);
    }

    /// Shared recoding used by the carry-policy test, over a u128 magnitude.
    fn recode_u128(value: u128, width: usize, windows: usize) -> (Vec<i32>, i32) {
        let limbs = [
            value as u64,
            (value >> 64) as u64,
            0,
            0,
            0,
            0,
        ];
        let mut out = alloc::vec![0i32; windows];
        let mut carry = 0i32;
        for (j, slot) in out.iter_mut().enumerate() {
            let offset = j * width;
            let limb = offset / 64;
            let shift = offset % 64;
            let mut word = limbs[limb] >> shift;
            if shift + width > 64 {
                word |= limbs[limb + 1] << (64 - shift);
            }
            let v = carry + (word & ((1u64 << width) - 1)) as i32;
            carry = (v + (1 << (width - 1))) >> width;
            *slot = v - (carry << width);
        }
        (out, carry)
    }

    /// The two-limb construction's premise, pinned as an inequality rather
    /// than a sample: the lattice constants *are* the reciprocals at scale
    /// 2^254, so there is no reciprocal table to get wrong.
    ///
    /// With 2^254 = n - delta, `2^254*v + n/2 - v*n = n/2 - v*delta`, so
    /// round-half-up(2^254*v/n) is v exactly when `v*delta <= n/2`. Derived
    /// here from `Fq::MODULUS` rather than copied from the module under test.
    /// (This replaces the 2^63 allowance check the three-limb reciprocal
    /// needed; that scale, and its error term, are gone.)
    #[test]
    fn the_pallas_lattice_constants_are_the_reciprocals_at_2_254() {
        use mina_curves::pasta::Fq;
        const A: u128 = 0x49e69d1640a899538cb1279300000000;
        const B: u128 = 0x49e69d1640f049157fcae1c700000001;
        // delta = n - 2^254, from the modulus itself.
        let delta = {
            let mut d = Fq::MODULUS;
            d.0[3] -= 1 << 62;
            assert_eq!(d.0[2], 0, "n - 2^254 must fit two limbs");
            assert_eq!(d.0[3], 0, "n - 2^254 must fit two limbs");
            [d.0[0], d.0[1]]
        };
        let half_n = {
            let mut h = Fq::MODULUS;
            h.div2();
            h
        };
        // A 2x2 schoolbook, kept local so the check shares no code with the
        // implementation it is checking.
        for (name, value) in [("A", A), ("B", B)] {
            let a = [value as u64, (value >> 64) as u64];
            let mut p = [0u64; 4];
            for (i, &x) in a.iter().enumerate() {
                let mut carry = 0u64;
                for (j, &y) in delta.iter().enumerate() {
                    let t = u128::from(p[i + j]) + u128::from(x) * u128::from(y) + u128::from(carry);
                    p[i + j] = t as u64;
                    carry = (t >> 64) as u64;
                }
                p[i + 2] = carry;
            }
            assert!(
                BigInt(p) <= half_n,
                "{name}*delta exceeded n/2, so round(2^254*{name}/n) is not {name}"
            );
        }
    }

    /// Second, independent coverage for `pallas_glv::decompose_bigint`.
    ///
    /// `cargo test -p mina-curves` refuses -- it is not a workspace member and
    /// needs dev-dependencies -- but the module's own tests do run, via the
    /// `#[path]` include in `tests/pallas_msm_spikes.rs`. So this is a second
    /// opinion rather than the only one, and it earns its place by deriving
    /// lambda and both bounds from `Fq::MODULUS` and the published lattice
    /// constants instead of importing them from the module under test.
    ///
    /// Checks the GLV relation itself -- k1 + lambda*k2 = k in the scalar
    /// field -- plus the magnitude bounds, over the extremes and several
    /// scalars that sit on a rounding boundary of one construction or another.
    #[test]
    fn pallas_decomposition_reconstructs_and_stays_bounded() {
        use ark_ff::{AdditiveGroup, Field};
        use mina_curves::pasta::{pallas_glv, Fq};
        // lambda = A / B in the scalar field, from the published constants;
        // derived here rather than copied from the module under test.
        const A: u128 = 0x49e69d1640a899538cb1279300000000;
        const B: u128 = 0x49e69d1640f049157fcae1c700000001;
        let lambda = Fq::from(A) * Fq::from(B).inverse().expect("B invertible");
        // The defining properties: a primitive cube root of unity, and the
        // lattice columns (A, -B) and (D, A) in the kernel of x + lambda*y.
        assert_ne!(lambda, Fq::ONE);
        assert_eq!(lambda.square() + lambda + Fq::ONE, Fq::ZERO);
        assert_eq!(Fq::from(A) - lambda * Fq::from(B), Fq::ZERO);
        assert_eq!(Fq::from(A + B) + lambda * Fq::from(A), Fq::ZERO);

        // Both bounds for the centred two-limb construction, derived here.
        // Only half_1 carries delta, in proportion to the centred scalar:
        // |half_1| <= k'*delta/2^254 + (A + D)/2 with k' <= (n - 1)/2, and
        // (A + D) odd makes the strict integer bound floor(..) + (A+D)/2 + 2.
        // half_2 is just B*eta1 - A*eta2, so (A + B)/2 + 1.
        //
        // The old three-limb bounds -- (A+D)/2 + 2^63 + 1 -- are NOT valid
        // here: this construction reaches 0.9899*2^127 in sampling against
        // their 0.8660*2^127, so asserting them would pass only by the
        // distribution and fail on an unlucky scalar.
        let delta = {
            let mut d = Fq::MODULUS;
            d.0[3] -= 1 << 62;
            [d.0[0], d.0[1]]
        };
        let half_modulus = {
            let mut h = Fq::MODULUS;
            h.div2();
            h
        };
        let half_1_bound = {
            let mut p = [0u64; 6];
            for (i, &x) in delta.iter().enumerate() {
                let mut carry = 0u64;
                for (j, &y) in half_modulus.0.iter().enumerate() {
                    let t = u128::from(p[i + j]) + u128::from(x) * u128::from(y) + u128::from(carry);
                    p[i + j] = t as u64;
                    carry = (t >> 64) as u64;
                }
                p[i + 4] = carry;
            }
            assert!(p[5] < 1 << 62, "(n-1)/2 * delta reached 2^380");
            let term = u128::from((p[3] >> 62) | (p[4] << 2))
                | (u128::from((p[4] >> 62) | (p[5] << 2)) << 64);
            term + (A + (A + B)) / 2 + 2
        };
        let half_2_bound: u128 = (A + B) / 2 + 1;
        assert!(half_1_bound < 1 << 127 && half_2_bound < 1 << 127);
        assert_eq!(
            half_1_bound, 0x7ffd381f65eb124d12ba0e0700000002,
            "the centred bound is 0.999915 of 2^127; this pins it against drift"
        );
        #[allow(non_snake_case)]
        let HALF_1_BOUND = half_1_bound;
        #[allow(non_snake_case)]
        let HALF_2_BOUND = half_2_bound;

        let signed = |(negative, magnitude): (bool, u128)| -> Fq {
            let v = Fq::from(magnitude);
            if negative {
                -v
            } else {
                v
            }
        };
        let n_minus_one = (-Fq::ONE).into_bigint();
        // n, n+1, 2n, 3n and 2^256-1 exercise the `while scalar >= MODULUS`
        // reduction, which every canonical input leaves dead. 3n is the most
        // subtractions the loop can need, since 4n exceeds 2^256.
        let modulus = Fq::MODULUS;
        let double = {
            let mut v = modulus;
            let _ = v.add_with_carry(&modulus);
            v
        };
        let triple = {
            let mut v = double;
            let _ = v.add_with_carry(&modulus);
            v
        };
        let mut vectors = alloc::vec![
            BigInt([0, 0, 0, 0]),
            BigInt([1, 0, 0, 0]),
            n_minus_one,
            BigInt([u64::MAX, u64::MAX, 0, 0]),
            BigInt([u64::MAX, u64::MAX >> 1, 0, 0]),
            // The two scalars whose product carried into the high limb of the
            // quotient at the retired 2^318 scale -- kept as coverage, since a
            // boundary for one construction is ordinary input for another.
            BigInt([0xecdf8dc411d55947, 0x1086ff3d8178809e, 0xddb3d742c2d0db40, 0]),
            BigInt([0x65088d26b4544cc7, 0x3739d0a181e407fb, 0xddb3d742c1f9cbfa, 0]),
            // Codex's counterexample to equivalence: the 2^318 and 2^384
            // reciprocals round this one differently, and the 2^254 scale is a
            // third answer again. All of them are bounded and recompose, which
            // is what this test checks.
            BigInt([
                0x0d7a6b1fbde9f2bf,
                0xed2de92e6fbc9fbf,
                0xe25ff90a1dd2fcf0,
                0x1ba314ac2bb60fb7,
            ]),
            // Noncanonical: reduced by decompose_bigint itself.
            modulus,
            {
                let mut v = modulus;
                let _ = v.add_with_carry(&BigInt([1, 0, 0, 0]));
                v
            },
            double,
            triple,
            BigInt([u64::MAX; 4]),
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..64 {
            let mut v = [0u64; 4];
            for slot in v.iter_mut() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *slot = state;
            }
            v[3] &= 0x3fff_ffff_ffff_ffff;
            vectors.push(BigInt(v));
        }
        let mut widest_1 = 0u128;
        let mut widest_2 = 0u128;
        for scalar in vectors {
            let (a, b) = pallas_glv::decompose_bigint(scalar);
            assert!(a.1 < HALF_1_BOUND, "k1 {:#x} exceeds its bound", a.1);
            assert!(b.1 < HALF_2_BOUND, "k2 {:#x} exceeds its bound", b.1);
            assert!(!a.0 || a.1 != 0, "no negative zero");
            assert!(!b.0 || b.1 != 0, "no negative zero");
            let k = Fq::from_bigint(scalar).unwrap_or_else(|| {
                // decompose_bigint reduces non-canonical input itself, so
                // compare against the reduced value.
                Fq::from_le_bytes_mod_order(&scalar.to_bytes_le())
            });
            assert_eq!(signed(a) + lambda * signed(b), k, "GLV relation failed");
            widest_1 = widest_1.max(a.1);
            widest_2 = widest_2.max(b.1);
        }
        // The vectors must actually stress the bounds, or they prove nothing.
        assert!(widest_1 > 1 << 125, "k1 never got large: {widest_1:#x}");
        assert!(widest_2 > 1 << 120, "k2 never got large: {widest_2:#x}");
    }

    // The strict magnitude bounds `vesta_glv::decompose` guarantees, which are
    // what licenses eliding the carry window at width 16. Exclusive.
    const HALF_1_MAX: u128 = 0x49e69d1640cc7134863e04ad00000001;
    const HALF_2_MAX: u128 = 0x6ed9eba16120bdde4c96987680000002;

    fn limbs_of(value: u128) -> BigInt<4> {
        BigInt([value as u64, (value >> 64) as u64, 0, 0])
    }

    /// The recoding, mirrored so a test can inspect the digits. Kept separate
    /// from the implementation on purpose: the reconstruction below is a check
    /// on the arithmetic, not on whether two copies agree.
    fn recode(scalar: &BigInt<4>, width: usize, windows: usize) -> (Vec<i32>, i32) {
        let limbs = [scalar.0[0], scalar.0[1], scalar.0[2], scalar.0[3], 0, 0];
        let mut out = vec![0i32; windows];
        let mut carry = 0i32;
        for (j, slot) in out.iter_mut().enumerate() {
            let offset = j * width;
            let limb = offset / 64;
            let shift = offset % 64;
            let mut word = limbs[limb] >> shift;
            if shift + width > 64 {
                word |= limbs[limb + 1] << (64 - shift);
            }
            let value = carry + (word & ((1u64 << width) - 1)) as i32;
            carry = (value + (1 << (width - 1))) >> width;
            *slot = value - (carry << width);
        }
        (out, carry)
    }

    /// Eight width-16 digits reconstruct every in-bound magnitude exactly, and
    /// every digit stays inside the signed window.
    #[test]
    fn eight_digits_reconstruct_the_bounded_glv_magnitudes() {
        let mut values = alloc::vec![0u128, 1, 2, 0x8000, 0xffff, 0x1_0000, u64::MAX as u128];
        for bound in [HALF_1_MAX, HALF_2_MAX] {
            for delta in 1..=4u128 {
                values.push(bound - delta);
            }
        }
        for value in values {
            let (digits, carry) = recode(&limbs_of(value), 16, 8);
            assert_eq!(carry, 0, "eight digits did not absorb {value:#x}");
            let mut reconstructed = 0i128;
            for (j, &digit) in digits.iter().enumerate() {
                assert!(
                    (-(1 << 15)..(1 << 15)).contains(&digit),
                    "digit {digit} outside the signed window for {value:#x}"
                );
                reconstructed += i128::from(digit) * (1i128 << (16 * j));
            }
            assert_eq!(
                reconstructed, value as i128,
                "signed-digit reconstruction failed for {value:#x}"
            );
        }
    }

    /// The counterexamples that show `bits = 128` alone does not license the
    /// elision: a merely 128-bit scalar can still carry out of the eighth
    /// width-16 window, and both of these do.
    #[test]
    fn generic_128_bit_scalars_still_need_the_ninth_digit() {
        for value in [(1u128 << 127) - 1, u128::MAX] {
            let (_, carry) = recode(&limbs_of(value), 16, 8);
            assert_ne!(
                carry, 0,
                "{value:#x} was expected to carry out of eight width-16 digits"
            );
            let (digits, carry) = recode(&limbs_of(value), 16, 9);
            assert_eq!(carry, 0);
            assert_ne!(digits[8], 0, "the ninth digit should be live for {value:#x}");
        }
    }

    /// Ten width-13 digits suffice for a 128-bit magnitude, which is what
    /// licenses the Pallas elision, and the top chunk cannot reach the carry
    /// threshold.
    #[test]
    fn ten_width_13_digits_absorb_any_128_bit_magnitude() {
        for value in [
            0u128,
            1,
            (1u128 << 127) - 1,
            u128::MAX,
            0x8000,
            u64::MAX as u128,
        ] {
            let (digits, carry) = recode(&limbs_of(value), 13, 10);
            assert_eq!(carry, 0, "ten width-13 digits did not absorb {value:#x}");
            assert!(digits.iter().all(|d| (-(1 << 12)..(1 << 12)).contains(d)));
        }
    }

    /// Eliding the carry window on in-bound scalars gives the same MSM as
    /// reserving it, zero and the most negative digit included.
    #[test]
    fn eliding_the_carry_window_does_not_change_the_result() {
        use mina_curves::pasta::point_spikes::{msm, CarryWindow, PastaCurve};
        let g = Vesta::generator();
        let bases: Vec<_> = (0..9)
            .map(|i| match i % 3 {
                0 => Vesta::zero(),
                1 => g,
                _ => (g * Fp::from(i + 2)).into_affine(),
            })
            .collect();
        // 0x8000 recodes to a digit of -32768, the negative endpoint of the
        // signed window, while leaving the top chunk zero.
        let mut scalars = alloc::vec![limbs_of(0), limbs_of(1), limbs_of(0x8000)];
        for bound in [HALF_1_MAX, HALF_2_MAX] {
            for delta in 1..=3u128 {
                scalars.push(limbs_of(bound - delta));
            }
        }
        scalars.resize(bases.len(), limbs_of(7));
        assert_eq!(
            msm::<VestaParameters>(&bases, &scalars, 16, 128, CarryWindow::Elided, PastaCurve::Vesta),
            msm::<VestaParameters>(&bases, &scalars, 16, 128, CarryWindow::Reserved, PastaCurve::Vesta),
        );
    }

    /// Asking for the elision when it is not justified aborts rather than
    /// returning a wrong sum. This is the guard that makes `Elided` safe to
    /// offer at all.
    #[test]
    #[should_panic(expected = "point MSM final carry")]
    fn eliding_the_carry_window_out_of_bounds_fails_closed() {
        use mina_curves::pasta::point_spikes::{msm, CarryWindow, PastaCurve};
        let bases = [Vesta::generator()];
        let scalars = [limbs_of((1u128 << 127) - 1)];
        let _ = msm::<VestaParameters>(&bases, &scalars, 16, 128, CarryWindow::Elided, PastaCurve::Vesta);
    }

    /// Why the non-GLV 255-bit Vesta call site keeps its carry window. At
    /// width 16 the top window covers bits 240..256, so a canonical scalar can
    /// present a chunk of 0x7fff with a carry of 1 into it, and 0x8000 carries
    /// out. `0x7fff_ffff << 224` is such a scalar and is below 2^255.
    #[test]
    #[should_panic(expected = "point MSM final carry")]
    fn eliding_at_255_bits_is_unsafe_and_fails_closed() {
        use mina_curves::pasta::point_spikes::{msm, CarryWindow, PastaCurve};
        let bases = [Vesta::generator()];
        let scalars = [BigInt([0, 0, 0, 0x7fff_ffff << 32])];
        assert!(scalars[0].num_bits() <= 255);
        let _ = msm::<VestaParameters>(&bases, &scalars, 16, 255, CarryWindow::Elided, PastaCurve::Vesta);
    }

    /// A scalar wider than the declared bound is rejected, not truncated,
    /// whichever carry policy is in force. 2^129 - 1 needs 129 bits.
    #[test]
    #[should_panic(expected = "exceeds declared bit bound")]
    fn a_scalar_above_the_declared_bound_is_rejected() {
        use mina_curves::pasta::point_spikes::{msm, CarryWindow, PastaCurve};
        let bases = [Vesta::generator()];
        let scalars = [BigInt([u64::MAX, u64::MAX, 1, 0])];
        assert_eq!(scalars[0].num_bits(), 129);
        let _ = msm::<VestaParameters>(&bases, &scalars, 16, 128, CarryWindow::Reserved, PastaCurve::Vesta);
    }

    /// r25 reset each bucket between windows by writing only the identity
    /// flag, on the argument that an identity bucket's eight coordinate words
    /// are never read. This is the targeted regression for that invariant: a
    /// bucket that is populated in one window, skipped in the next so it
    /// carries stale coordinates under an identity flag, and then either
    /// populated again or left empty through the summation pass. Either read
    /// of the stale words would land a wrong point in the sum.
    #[test]
    fn stale_bucket_coordinates_survive_a_flag_only_reset() {
        // The recoding from point_spikes::msm, duplicated so the test can see
        // which bucket each window touches. Kept deliberately separate: if the
        // two ever disagree, the pattern assertions below stop being about the
        // code under test and the test says so by failing to find a pattern.
        fn recode(scalar: &BigInt<4>, width: usize, windows: usize) -> Vec<i32> {
            let limbs = [scalar.0[0], scalar.0[1], scalar.0[2], scalar.0[3], 0, 0];
            let mut out = vec![0i32; windows];
            let mut carry = 0i32;
            for (j, slot) in out.iter_mut().enumerate() {
                let offset = j * width;
                let limb = offset / 64;
                let shift = offset % 64;
                let mut word = limbs[limb] >> shift;
                if shift + width > 64 {
                    word |= limbs[limb + 1] << (64 - shift);
                }
                let value = carry + (word & ((1u64 << width) - 1)) as i32;
                carry = (value + (1 << (width - 1))) >> width;
                *slot = value - (carry << width);
            }
            assert_eq!(carry, 0, "test recoding disagrees with the implementation");
            out
        }

        let width = 3usize;
        let bits = 9usize;
        let windows = bits.div_ceil(width) + 1;
        let buckets = 1usize << (width - 1);
        let candidates: Vec<(BigInt<4>, Vec<i32>)> = (0u64..1 << bits)
            .map(|v| {
                let scalar = BigInt([v, 0, 0, 0]);
                let digits = recode(&scalar, width, windows);
                (scalar, digits)
            })
            .collect();

        // Occupancy of bucket b at window j for a pair of scalars. Windows are
        // consumed from the top down, so window j+1 runs before window j: a
        // bucket touched at j+1 and skipped at j holds stale coordinates for
        // the whole of window j.
        let occupied = |digits: &[&Vec<i32>], j: usize, b: usize| {
            digits
                .iter()
                .any(|d| d[j] != 0 && d[j].unsigned_abs() as usize - 1 == b)
        };

        let mut gap_then_reuse = None;
        let mut gap_then_empty = None;
        for (s0, d0) in &candidates {
            for (s1, d1) in &candidates {
                let pair = [d0, d1];
                let top = (0..windows)
                    .rev()
                    .find(|&j| d0[j] != 0 || d1[j] != 0)
                    .unwrap_or(0);
                for b in 0..buckets {
                    for j in (1..top).rev() {
                        if !occupied(&pair, j + 1, b) || occupied(&pair, j, b) {
                            continue;
                        }
                        // Stale at window j. Touched again below it, or not.
                        if (0..j).any(|k| occupied(&pair, k, b)) {
                            gap_then_reuse.get_or_insert((*s0, *s1, b, j));
                        } else {
                            gap_then_empty.get_or_insert((*s0, *s1, b, j));
                        }
                    }
                }
                if gap_then_reuse.is_some() && gap_then_empty.is_some() {
                    break;
                }
            }
            if gap_then_reuse.is_some() && gap_then_empty.is_some() {
                break;
            }
        }

        let (reuse_a, reuse_b, _, _) =
            gap_then_reuse.expect("no bucket is populated, skipped, then populated again");
        let (empty_a, empty_b, _, _) =
            gap_then_empty.expect("no bucket is populated, skipped, then left empty");

        let g = Vesta::generator();
        let bases = [g, (g + g).into_affine()];
        for scalars in [[reuse_a, reuse_b], [empty_a, empty_b]] {
            assert_eq!(
                mina_curves::pasta::point_spikes::msm::<VestaParameters>(
                    &bases,
                    &scalars,
                    width,
                    bits,
                    mina_curves::pasta::point_spikes::CarryWindow::Reserved,
                    mina_curves::pasta::point_spikes::PastaCurve::Vesta,
                )
                .into_affine(),
                projective_msm::<VestaParameters>(&bases, &scalars, width, bits).into_affine(),
                "flag-only bucket reset leaked stale coordinates"
            );
        }
    }

    #[test]
    fn shortened_windows_and_glv_match_reference() {
        let g = Vesta::generator();
        let bases: Vec<_> = (0..49)
            .map(|i| match i % 7 {
                0 => Vesta::zero(),
                1 => g,
                2 => -g,
                _ => (g * Fp::from(i + 1)).into_affine(),
            })
            .collect();
        for bits in [64, 127, 128, 255] {
            let scalars: Vec<_> = (0..bases.len())
                .map(|i| {
                    let mut limbs = [u64::MAX; 4];
                    for bit in bits..256 {
                        limbs[bit / 64] &= !(1 << (bit % 64));
                    }
                    limbs[0] -= i as u64;
                    if bits == 255 {
                        limbs[3] >>= 1;
                    }
                    BigInt(limbs)
                })
                .collect();
            let fields: Vec<_> = scalars
                .iter()
                .map(|s| Fp::from_bigint(*s).unwrap())
                .collect();
            let expected = bases
                .iter()
                .zip(&fields)
                .fold(Projective::<VestaParameters>::zero(), |s, (p, k)| {
                    s + *p * k
                });
            for width in [3, 5, 8, 12] {
                assert_eq!(
                    projective_msm::<VestaParameters>(&bases, &scalars, width, bits),
                    expected
                );
            }
            let (expanded, halves) = expand_glv::<VestaParameters>(
                &bases,
                &fields,
                crate::vesta_glv::endomorphism_coefficient(),
            );
            assert_eq!(
                projective_msm::<VestaParameters>(&expanded, &halves, 5, 128),
                expected
            );
        }
    }
}
