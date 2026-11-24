//! Experimental canonical-residue backend for the Vesta accumulator MSM.
//!
//! The protocol's fields, serialized inputs, SRS blob, and verification equations
//! remain unchanged. Only this MSM's base-field coordinates use canonical limbs:
//! bases enter once and the result leaves once. This avoids the second bigint
//! syscall needed to restore Montgomery scaling after every field product.
//!
//! We implement `FpConfig`, not `MontConfig`: the vendored Montgomery backend has
//! its own Pasta overrides, and treating canonical limbs as Montgomery limbs
//! would silently corrupt arithmetic. Arkworks still supplies point formulas,
//! identity/exception handling, and the same serial MSM algorithm as the baseline.

use alloc::vec::Vec;
use core::marker::PhantomData;

use ark_ec::{
    models::short_weierstrass::{Affine, Projective, SWCurveConfig},
    scalar_mul::variable_base::msm_bigint_serial,
    CurveConfig,
};
use ark_ff::{
    BigInt, BigInteger, Field, Fp as FieldElement, FpConfig, PrimeField, SqrtPrecomputation,
};
use mina_curves::pasta::{Fp, Fq, Vesta, VestaParameters};

type CanonicalFq = FieldElement<CanonicalFqConfig, 4>;

struct CanonicalFqConfig;

const fn canonical(limbs: [u64; 4]) -> CanonicalFq {
    FieldElement(BigInt(limbs), PhantomData)
}

fn from_fq(value: Fq) -> CanonicalFq {
    FieldElement(value.into_bigint(), PhantomData)
}

fn to_fq(value: CanonicalFq) -> Fq {
    Fq::from_bigint(value.0).expect("canonical MSM field element is reduced")
}

impl FpConfig<4> for CanonicalFqConfig {
    const MODULUS: BigInt<4> = Fq::MODULUS;
    const GENERATOR: CanonicalFq = canonical([5, 0, 0, 0]);
    const ZERO: CanonicalFq = canonical([0; 4]);
    const ONE: CanonicalFq = canonical([1, 0, 0, 0]);
    const TWO_ADICITY: u32 = 32;
    // 5^((q - 1) / 2^32) mod q, in canonical little-endian limbs.
    const TWO_ADIC_ROOT_OF_UNITY: CanonicalFq = canonical([
        0xa70e2c1102b6d05f,
        0x9bb97ea3c106f049,
        0x9e5c4dfd492ae26e,
        0x2de6a9b8746d3f58,
    ]);
    const SQRT_PRECOMP: Option<SqrtPrecomputation<CanonicalFq>> =
        Some(SqrtPrecomputation::TonelliShanks {
            two_adicity: Self::TWO_ADICITY,
            quadratic_nonresidue_to_trace: Self::TWO_ADIC_ROOT_OF_UNITY,
            trace_of_modulus_minus_one_div_two: &[
                0x04ca546ec6237590,
                0x0000000011234c7e,
                0,
                0x0000000020000000,
            ],
        });

    #[inline]
    fn add_assign(a: &mut CanonicalFq, b: &CanonicalFq) {
        // q < 2^255, so the sum of two reduced residues cannot overflow.
        a.0.add_with_carry(&b.0);
        if a.0 >= Self::MODULUS {
            a.0.sub_with_borrow(&Self::MODULUS);
        }
    }

    #[inline]
    fn sub_assign(a: &mut CanonicalFq, b: &CanonicalFq) {
        if a.0 < b.0 {
            a.0.add_with_carry(&Self::MODULUS);
        }
        a.0.sub_with_borrow(&b.0);
    }

    #[inline]
    fn double_in_place(a: &mut CanonicalFq) {
        let b = *a;
        Self::add_assign(a, &b);
    }

    #[inline]
    fn neg_in_place(a: &mut CanonicalFq) {
        if a.0 != BigInt([0; 4]) {
            let mut neg = Self::MODULUS;
            neg.sub_with_borrow(&a.0);
            a.0 = neg;
        }
    }

    #[inline]
    fn mul_assign(a: &mut CanonicalFq, b: &CanonicalFq) {
        #[cfg(target_os = "zkvm")]
        {
            if matches!(option_env!("MIZU_DIRECT_MULMOD"), Some("1")) {
                // UINT256_MUL reads eight words starting at its second pointer:
                // four RHS limbs followed by four modulus limbs. An array of
                // arrays guarantees adjacency and u64 alignment. Copy the RHS
                // before mutating `a`, including when squaring.
                let rhs_modulus = [(b.0).0, Self::MODULUS.0];
                // SAFETY: both arrays are initialized and 8-byte aligned. The
                // full 64-byte RHS/modulus buffer is alive and disjoint from
                // `a`; the syscall writes the reduced result into `a` in place.
                unsafe {
                    if matches!(option_env!("MIZU_PASTA_FP_PRECOMPILE"), Some("1")) {
                        // CanonicalFq: the Vesta base field.
                        pasta_fp_mul::<true>(&mut (a.0).0, &(b.0).0);
                    } else {
                        sp1_lib::syscall_uint256_mulmod(&mut (a.0).0, rhs_modulus.as_ptr());
                    }
                }
                return;
            }
            let mut result = [0u64; 4];
            // SAFETY: every argument is an initialized, 8-byte-aligned array
            // of four u64 limbs; the output is separate from the inputs, and
            // the fixed nonzero modulus and both operands are canonical.
            unsafe {
                sp1_lib::sys_bigint(&mut result, 0, &(a.0).0, &(b.0).0, &Self::MODULUS.0);
            }
            a.0 = BigInt(result);
        }
        #[cfg(not(target_os = "zkvm"))]
        {
            // Native differential tests exercise this representation and the
            // identical curve/MSM schedule; actual syscall cost is guest-only.
            *a = from_fq(to_fq(*a) * to_fq(*b));
        }
    }

    #[inline]
    fn sum_of_products<const M: usize>(a: &[CanonicalFq; M], b: &[CanonicalFq; M]) -> CanonicalFq {
        let mut result = Self::ZERO;
        for i in 0..M {
            let mut product = a[i];
            Self::mul_assign(&mut product, &b[i]);
            Self::add_assign(&mut result, &product);
        }
        result
    }

    #[inline]
    fn square_in_place(a: &mut CanonicalFq) {
        let b = *a;
        Self::mul_assign(a, &b);
    }

    fn inverse(a: &CanonicalFq) -> Option<CanonicalFq> {
        #[cfg(target_os = "zkvm")]
        if mina_curves::pasta::precompile_spikes::hinted_inverse_enabled() {
            return mina_curves::pasta::precompile_spikes::hinted_inverse(*a);
        }
        // Affine batch reduction inverts one product per tree level. Keep the
        // existing binary inverse when the checked-hint experiment is disabled.
        to_fq(*a).inverse().map(from_fq)
    }

    #[inline]
    fn from_bigint(value: BigInt<4>) -> Option<CanonicalFq> {
        (value < Self::MODULUS).then_some(FieldElement(value, PhantomData))
    }

    #[inline]
    fn into_bigint(value: CanonicalFq) -> BigInt<4> {
        value.0
    }
}

struct CanonicalVestaParameters;

impl CurveConfig for CanonicalVestaParameters {
    type BaseField = CanonicalFq;
    type ScalarField = Fp;
    const COFACTOR: &'static [u64] = VestaParameters::COFACTOR;
    const COFACTOR_INV: Fp = VestaParameters::COFACTOR_INV;
}

impl SWCurveConfig for CanonicalVestaParameters {
    const COEFF_A: CanonicalFq = CanonicalFqConfig::ZERO;
    const COEFF_B: CanonicalFq = canonical([5, 0, 0, 0]);
    const GENERATOR: Affine<Self> = Affine::new_unchecked(
        CanonicalFqConfig::ONE,
        canonical([
            0x4e4389b9b0276a62,
            0xacce3a7f298ba20c,
            0x13b64e3aae89754c,
            0x1943666ea922ae6b,
        ]),
    );
}

/// Compute the same Vesta MSM as arkworks, with canonical coordinate storage.
///
/// Input points retain their existing provenance and validation. Conversion
/// copies coordinates and the identity flag; it never changes the equation or
/// turns unchecked bytes into accepted points. Mismatched lengths are rejected.
pub(crate) fn vesta_msm(bases: &[Vesta], scalars: &[Fp]) -> Projective<VestaParameters> {
    assert_eq!(bases.len(), scalars.len(), "canonical MSM input lengths");
    let fuse_conversion = matches!(option_env!("MIZU_FUSE_VESTA_BASE_CONVERSION"), Some("1"))
        && matches!(option_env!("MIZU_VESTA_GLV"), Some("1"))
        && matches!(option_env!("MIZU_VESTA_POINT_PRECOMPILE"), Some("1"))
        && option_env!("MIZU_VESTA_PREPARED_LANES").is_none();
    let canonical_bases: Vec<Affine<CanonicalVestaParameters>> = if fuse_conversion {
        Vec::new()
    } else { bases.iter()
        .map(|base| Affine {
            x: from_fq(base.x),
            y: from_fq(base.y),
            infinity: base.infinity,
        })
        .collect() };
    // GLV performs its own scalar-to-integer conversion. Keep the old path as
    // the control, but omit this unused vector (and its conversion syscalls)
    // when the explicitly selected GLV-only trimming experiment is active.
    let bigints: Vec<_> = if matches!(option_env!("MIZU_TRIM_CONVERSIONS"), Some("1"))
        && matches!(option_env!("MIZU_VESTA_GLV"), Some("1"))
        && option_env!("MIZU_VESTA_PREPARED_LANES").is_none()
    {
        Vec::new()
    } else {
        scalars.iter().map(|scalar| scalar.into_bigint()).collect()
    };
    let result = if option_env!("MIZU_VESTA_PREPARED_LANES").is_some() {
        assert!(
            !matches!(option_env!("MIZU_VESTA_GLV"), Some("1")),
            "prepared and GLV are separate experiments"
        );
        let (points, coefficients, bits) = prepared_inputs(&canonical_bases, &bigints);
        crate::vesta_spikes::run::<CanonicalVestaParameters>(
            &points,
            &coefficients,
            bits,
            // No magnitude bound is available on this path.
            None,
        )
    } else if matches!(option_env!("MIZU_VESTA_GLV"), Some("1")) {
        let beta = from_fq(crate::vesta_glv::endomorphism_coefficient());
        if matches!(option_env!("MIZU_VESTA_POINT_PRECOMPILE"), Some("1")) {
            // Convert each expanded point as it is produced, rather than
            // collecting a Vec<Affine> for the MSM to read back.
            if fuse_conversion {
                crate::vesta_spikes::run_glv_prepared_with_conversion::<CanonicalVestaParameters, _, _>(
                    bases, scalars, beta, |base| Affine {
                        x: from_fq(base.x), y: from_fq(base.y), infinity: base.infinity,
                    },
                )
            } else {
                crate::vesta_spikes::run_glv_prepared::<CanonicalVestaParameters>(
                    &canonical_bases, scalars, beta,
                )
            }
        } else {
            let (points, halves) = crate::vesta_spikes::expand_glv::<CanonicalVestaParameters>(
                &canonical_bases,
                scalars,
                beta,
            );
            crate::vesta_spikes::run::<CanonicalVestaParameters>(
                &points,
                &halves,
                128,
                Some(crate::vesta_glv::HALF_MAX),
            )
        }
    } else if matches!(option_env!("MIZU_VESTA_AFFINE"), Some("1"))
        || matches!(option_env!("MIZU_VESTA_COMPACT"), Some("1"))
        || matches!(option_env!("MIZU_VESTA_POINT_PRECOMPILE"), Some("1"))
    {
        crate::vesta_spikes::run::<CanonicalVestaParameters>(
            &canonical_bases,
            &bigints,
            255,
            // Canonical field elements, with no bound tighter than the
            // modulus: at width 16 and bits 255 the top window covers bits
            // 240..256, so a scalar can present 0x7fff with a carry into it.
            // `eliding_at_255_bits_is_unsafe_and_fails_closed` pins that.
            None,
        )
    } else {
        msm_bigint_serial::<Projective<CanonicalVestaParameters>>(&canonical_bases, &bigints)
    };
    Projective::new_unchecked(to_fq(result.x), to_fq(result.y), to_fq(result.z))
}

/// Closed canonical coefficients bypass the per-scalar Montgomery conversion.
/// This path uses the same GLV bounds, term builder, points and MSM as vesta_msm.
pub(crate) fn vesta_msm_canonical_scalars(
    bases: &[Vesta], scalars: &[mina_curves::pasta::canonical_words::CanonicalPasta<false>],
) -> Projective<VestaParameters> {
    assert_eq!(bases.len(), scalars.len(), "canonical MSM input lengths");
    let beta = from_fq(crate::vesta_glv::endomorphism_coefficient());
    let result = crate::vesta_spikes::run_glv_prepared_canonical_with_conversion::<CanonicalVestaParameters, _, _>(
        bases, scalars, beta, |base| Affine {
            x: from_fq(base.x), y: from_fq(base.y), infinity: base.infinity,
        },
    );
    Projective::new_unchecked(to_fq(result.x), to_fq(result.y), to_fq(result.z))
}

/// The private embedded table uses two unsigned128bit positional lanes.
/// Reserve the carry window: independent low-lane elision is invalid.
pub(crate) fn vesta_msm_fixed(
    table: &crate::fixed_srs::FixedVestaSrs<'_>,
    scalars: &[mina_curves::pasta::canonical_words::CanonicalPasta<false>],
) -> Projective<VestaParameters> {
    use mina_curves::pasta::point_spikes::{PreparedTerms, PastaCurve, CarryWindow, msm_prepared};
    assert_eq!(table.len(), scalars.len(), "fixed SRS scalar count");
    if matches!(option_env!("MIZU_FIXED_VESTA_COUPLED"), Some("1")) {
        let coupled_width = match (option_env!("MIZU_VESTA_WINDOW"), table.lane_shift()) {
            (Some("16"), 128) => Some(16usize),
            (Some("15"), 120) => Some(15usize),
            (Some("14"), 126) => Some(14usize),
            _ => None,
        };
        if let Some(width) = coupled_width {
            if matches!(option_env!("MIZU_BORROW_POSITIONAL_SCALARS"), Some("1")) {
                if let Some(coords) = table.finite_coords() {
                    let result = mina_curves::pasta::point_spikes::msm_vesta_finite_borrowed::<
                        CanonicalVestaParameters,
                    >(coords, scalars, width, table.lane_shift());
                    return Projective::new_unchecked(to_fq(result.x), to_fq(result.y), to_fq(result.z));
                }
                if width == 16 {
                    return vesta_msm_fixed_borrowed(table, scalars);
                }
            }
            if width == 16 {
                return vesta_msm_fixed_coupled(table, scalars);
            }
        }
    }
    assert_eq!(
        table.lane_shift(),
        128,
        "uncoupled fixed Vesta fallback requires 128-bit lane shift"
    );
    let mut terms = PreparedTerms::<CanonicalVestaParameters>::with_capacity(2 * scalars.len(), PastaCurve::Vesta);
    for lane in 0..2 {
        for (i, scalar) in scalars.iter().enumerate() {
            let words = scalar.limbs();
            let value = BigInt([words[2 * lane], words[2 * lane + 1], 0, 0]);
            if value.is_zero() { continue; }
            let point = table.point::<CanonicalVestaParameters>(lane, i);
            if !point.infinity { terms.push(&point, value); }
        }
    }
    let width = crate::vesta_spikes::window(terms.len());
    let result = msm_prepared(&terms, width, 128, CarryWindow::Reserved);
    Projective::new_unchecked(to_fq(result.x), to_fq(result.y), to_fq(result.z))
}

/// Coupled width16 variant of the immutable positional table path.
fn vesta_msm_fixed_coupled(
    table: &crate::fixed_srs::FixedVestaSrs<'_>,
    scalars: &[mina_curves::pasta::canonical_words::CanonicalPasta<false>],
) -> Projective<VestaParameters> {
    use mina_curves::pasta::point_spikes::{PreparedTerms, PastaCurve, msm_prepared_vesta_positional};
    assert_eq!(table.len(), scalars.len(), "fixed SRS scalar count");
    let mut terms = PreparedTerms::<CanonicalVestaParameters>::with_capacity(2 * scalars.len(), PastaCurve::Vesta);
    for lane in 0..2 {
        for (i, scalar) in scalars.iter().enumerate() {
            let words = scalar.limbs();
            let value = BigInt([words[2*lane], words[2*lane+1], 0, 0]);
            let point = table.point::<CanonicalVestaParameters>(lane, i);
            // Keep even zero/identity slots: positional pairing is essential.
            terms.push(&point, value);
        }
    }
    let result = msm_prepared_vesta_positional(&terms);
    Projective::new_unchecked(to_fq(result.x), to_fq(result.y), to_fq(result.z))
}

/// Borrow the closed canonical coefficients; only points/negations materialize.
fn vesta_msm_fixed_borrowed(
    table: &crate::fixed_srs::FixedVestaSrs<'_>,
    scalars: &[mina_curves::pasta::canonical_words::CanonicalPasta<false>],
) -> Projective<VestaParameters> {
    use mina_curves::pasta::point_spikes::{PreparedVestaPositional, msm_prepared_vesta_positional_borrowed};
    assert_eq!(table.len(), scalars.len(), "fixed SRS scalar count");
    let mut terms = PreparedVestaPositional::<CanonicalVestaParameters>::with_scalars(scalars);
    for lane in 0..2 {
        for i in 0..scalars.len() {
            let point = table.point::<CanonicalVestaParameters>(lane, i);
            terms.push(&point);
        }
    }
    let result = msm_prepared_vesta_positional_borrowed(&terms);
    Projective::new_unchecked(to_fq(result.x), to_fq(result.y), to_fq(result.z))
}

/// Positional fixed-base tables are generated from the same SRS on the host and
/// baked into the ELF. Check the first layer against every supplied base, so the
/// table cannot silently be applied to a different verifier/SRS. All remaining
/// layers are derived by the build artifact generator, not provided in stdin.
fn prepared_inputs(
    bases: &[Affine<CanonicalVestaParameters>],
    scalars: &[BigInt<4>],
) -> (Vec<Affine<CanonicalVestaParameters>>, Vec<BigInt<4>>, usize) {
    let lanes = match option_env!("MIZU_VESTA_PREPARED_LANES") {
        Some("2") => 2,
        Some("4") => 4,
        _ => panic!("MIZU_VESTA_PREPARED_LANES must be 2 or 4"),
    };
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/vesta-prepared.bin"));
    let word = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
    assert!(
        bytes.len() >= 24 && &bytes[..8] == b"ZKVESTA1",
        "missing prepared Vesta artifact"
    );
    assert_eq!(word(8) as usize, lanes, "prepared lane count");
    assert_eq!(word(16) as usize, bases.len(), "prepared SRS length");
    assert_eq!(
        bytes.len(),
        24 + lanes * bases.len() * 64,
        "prepared artifact length"
    );
    let bits = 256 / lanes;
    let mut points = Vec::with_capacity(lanes * bases.len());
    let mut coefficients = Vec::with_capacity(lanes * bases.len());
    for lane in 0..lanes {
        for (i, scalar) in scalars.iter().enumerate() {
            let at = 24 + (lane * bases.len() + i) * 64;
            let x = BigInt(core::array::from_fn(|j| word(at + j * 8)));
            let y = BigInt(core::array::from_fn(|j| word(at + 32 + j * 8)));
            let point = Affine::new_unchecked(
                CanonicalFq::from_bigint(x).expect("prepared x reduced"),
                CanonicalFq::from_bigint(y).expect("prepared y reduced"),
            );
            if lane == 0 {
                assert_eq!(point, bases[i], "prepared SRS binding");
            }
            let mut value = [0; 4];
            value[..bits / 64]
                .copy_from_slice(&scalar.0[lane * (bits / 64)..(lane + 1) * (bits / 64)]);
            if value != [0; 4] {
                points.push(point);
                coefficients.push(BigInt(value));
            }
        }
    }
    (points, coefficients, bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ec::{AffineRepr, CurveGroup};
    use ark_ff::{AdditiveGroup, FftField, Zero};
    use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};

    #[test]
    fn field_constants_and_encoding_match_reference() {
        assert_eq!(to_fq(CanonicalFq::ZERO), Fq::ZERO);
        assert_eq!(to_fq(CanonicalFq::ONE), Fq::ONE);
        assert_eq!(to_fq(CanonicalFq::GENERATOR), Fq::GENERATOR);
        assert_eq!(
            to_fq(CanonicalFq::TWO_ADIC_ROOT_OF_UNITY),
            Fq::TWO_ADIC_ROOT_OF_UNITY
        );
        assert!(CanonicalFq::from_bigint(Fq::MODULUS).is_none());
        assert!(CanonicalFq::from_bigint(BigInt([u64::MAX; 4])).is_none());
        let generator = CanonicalVestaParameters::GENERATOR;
        assert!(generator.is_on_curve());
        assert_eq!(to_fq(generator.x), Vesta::generator().x);
        assert_eq!(to_fq(generator.y), Vesta::generator().y);

        for value in field_cases() {
            let canonical = from_fq(value);
            let mut reference_bytes = Vec::new();
            value.serialize_compressed(&mut reference_bytes).unwrap();
            let mut canonical_bytes = Vec::new();
            canonical
                .serialize_compressed(&mut canonical_bytes)
                .unwrap();
            assert_eq!(canonical_bytes, reference_bytes);
            let decoded = CanonicalFq::deserialize_compressed(canonical_bytes.as_slice()).unwrap();
            assert_eq!(decoded, canonical);
        }
    }

    fn field_cases() -> Vec<Fq> {
        let mut cases = alloc::vec![Fq::ZERO, Fq::ONE, -Fq::ONE, Fq::from(2), Fq::from(u64::MAX)];
        for i in [63, 64, 127, 128, 191, 192, 253] {
            let value = Fq::from(2).pow([i]);
            cases.extend([value - Fq::ONE, value, value + Fq::ONE]);
        }
        let mut value = Fq::from(123456789);
        for i in 0..32 {
            value = value.square() + Fq::from(i + 17);
            cases.push(value);
        }
        cases
    }

    #[test]
    fn canonical_field_operations_match_montgomery_reference() {
        let cases = field_cases();
        for &a in &cases {
            let ca = from_fq(a);
            assert_eq!(to_fq(-ca), -a);
            assert_eq!(to_fq(ca.double()), a.double());
            assert_eq!(to_fq(ca.square()), a.square());
            assert_eq!(ca.inverse().map(to_fq), a.inverse());
            assert_eq!(ca.square().sqrt().unwrap().square(), ca.square());
            for &b in &cases {
                let cb = from_fq(b);
                assert_eq!(to_fq(ca + cb), a + b);
                assert_eq!(to_fq(ca - cb), a - b);
                assert_eq!(to_fq(ca * cb), a * b);
                assert_eq!(
                    to_fq(CanonicalFq::sum_of_products(&[ca, cb], &[cb, ca])),
                    a * b + b * a
                );
            }
        }
    }

    #[test]
    fn canonical_msm_matches_reference_with_exceptional_points_and_scalars() {
        let generator = Vesta::generator();
        for len in [0, 1, 2, 31, 32, 257] {
            let bases: Vec<_> = (0..len)
                .map(|i| match i % 7 {
                    0 => Vesta::zero(),
                    1 | 2 => generator,
                    3 => -generator,
                    _ => (generator * Fp::from(i as u64 + 1)).into_affine(),
                })
                .collect();
            let scalars: Vec<_> = (0..len)
                .map(|i| match i % 5 {
                    0 => Fp::ZERO,
                    1 => Fp::ONE,
                    2 => -Fp::ONE,
                    _ => Fp::from(i as u64 + 9).pow([17]),
                })
                .collect();
            let reference = bases.iter().zip(&scalars).fold(
                Projective::<VestaParameters>::zero(),
                |sum, (base, scalar)| sum + *base * scalar,
            );
            assert_eq!(vesta_msm(&bases, &scalars), reference, "length {len}");
            let canonical: Vec<_> = scalars.iter().map(|s|
                mina_curves::pasta::canonical_words::CanonicalPasta::from_limbs(s.into_bigint().0).unwrap()
            ).collect();
            assert_eq!(vesta_msm_canonical_scalars(&bases, &canonical), reference, "canonical scalar length {len}");
            for (scalar, canonical) in scalars.iter().zip(&canonical) {
                assert_eq!(crate::vesta_glv::decompose(*scalar), crate::vesta_glv::decompose_canonical(*canonical));
            }
        }
    }

    #[test]
    #[should_panic(expected = "canonical MSM input lengths")]
    fn canonical_msm_rejects_mismatched_lengths() {
        let _ = vesta_msm(&[Vesta::generator()], &[]);
    }

    #[test]
    fn coupled_table_path_matches_native_group_sum_at_carry_boundary() {
        use mina_curves::pasta::canonical_words::CanonicalPasta;
        let bases = [Vesta::generator(), -Vesta::generator(), Vesta::zero()];
        let bytes = crate::fixed_srs::encode(&bases);
        let table = crate::fixed_srs::decode(&bytes, bases.len());
        let boundary = BigInt([0x7fff7fff7fff8000,0x7fff7fff7fff7fff,0,0]);
        let scalars = [CanonicalPasta::from_limbs(boundary.0).unwrap(),
            CanonicalPasta::from_limbs((-Fp::ONE).into_bigint().0).unwrap(),
            CanonicalPasta::from_limbs([0,0,1,0]).unwrap()];
        let expected = bases.iter().zip(&scalars).fold(Projective::<VestaParameters>::zero(),
            |sum,(p,k)| sum + p.mul_bigint(BigInt(k.limbs())));
        assert_eq!(vesta_msm_fixed_coupled(&table, &scalars), expected);
        assert_eq!(vesta_msm_fixed_borrowed(&table, &scalars), expected);
    }

    #[test]
    fn positional_table_msm_matches_integer_group_sum_at_carry_boundaries() {
        use mina_curves::pasta::canonical_words::CanonicalPasta;
        let g = Vesta::generator();
        let bases = [g, -g, Vesta::zero(), (g * Fp::from(17u64)).into_affine()];
        let bytes = crate::fixed_srs::encode(&bases);
        let table = crate::fixed_srs::decode(&bytes, bases.len());
        let cases = [Fp::ZERO, Fp::ONE, -Fp::ONE,
            Fp::from(2u64).pow([127]) - Fp::ONE,
            Fp::from(2u64).pow([127]), Fp::from(2u64).pow([128]) - Fp::ONE,
            Fp::from(2u64).pow([128]), Fp::from(2u64).pow([254])];
        for i in 0..cases.len() {
            let scalars: Vec<_> = (0..bases.len()).map(|j| cases[(i + j) % cases.len()]).collect();
            let canonical: Vec<_> = scalars.iter().map(|s| CanonicalPasta::from_limbs(s.into_bigint().0).unwrap()).collect();
            let expected = bases.iter().zip(&scalars).fold(Projective::<VestaParameters>::zero(),
                |sum,(p,k)| sum + *p * k);
            assert_eq!(vesta_msm_fixed(&table, &canonical), expected);
        }
    }

    #[test]
    #[should_panic(expected = "canonical MSM input lengths")]
    fn canonical_scalar_msm_rejects_mismatched_lengths() {
        let _ = vesta_msm_canonical_scalars(&[Vesta::generator()], &[]);
    }

    /// `chunked_multi_scalar_mul_verifier` folds `zeta_n^chunk` into each
    /// term's scalar so the kimchi verifier can build `ft` in one multiexp
    /// instead of an MSM per chunk index plus a projective Horner fold. It has
    /// to agree with the path it replaced, and the only place the bookkeeping
    /// can go wrong is commitments with *different* chunk counts, so the
    /// fixture mixes the shapes the verifier actually mixes: a single-chunk
    /// column commitment and a seven-chunk quotient commitment.
    #[test]
    fn a_fused_chunked_msm_equals_multi_scalar_mul_then_chunk_commitment() {
        use ark_ec::{short_weierstrass::Projective, PrimeGroup};
        use mina_curves::pasta::VestaParameters;
        use poly_commitment::commitment::PolyComm;

        let gen = Projective::<VestaParameters>::generator();
        let mut next = 0u64;
        let mut make = |n: usize| {
            PolyComm::new(
                (0..n)
                    .map(|_| {
                        next += 1;
                        (gen * Fp::from(next)).into_affine()
                    })
                    .collect(),
            )
        };
        let a = make(1);
        let b = make(3);
        let c = make(7);
        let comms: Vec<&PolyComm<Vesta>> = vec![&a, &b, &c];
        let scalars = vec![Fp::from(11u64), Fp::from(13u64), -Fp::from(17u64)];
        let zeta_n = Fp::from(23u64);

        let two_step = PolyComm::multi_scalar_mul(&comms, &scalars).chunk_commitment(zeta_n);
        let fused = PolyComm::chunked_multi_scalar_mul_verifier(&comms, &scalars, zeta_n);

        assert_eq!(two_step.chunks.len(), 1, "chunk_commitment collapses to one");
        assert_eq!(fused, two_step);
    }

    /// Changing `zeta_n` has to change the answer, or the test above would
    /// pass against an implementation that ignored the chunk weighting.
    #[test]
    fn a_fused_chunked_msm_actually_uses_the_chunk_weight() {
        use ark_ec::{short_weierstrass::Projective, PrimeGroup};
        use mina_curves::pasta::VestaParameters;
        use poly_commitment::commitment::PolyComm;

        let gen = Projective::<VestaParameters>::generator();
        let a: PolyComm<Vesta> = PolyComm::new(
            (1..=3u64)
                .map(|i| (gen * Fp::from(i)).into_affine())
                .collect(),
        );
        let comms = vec![&a];
        let scalars = vec![Fp::from(7u64)];

        let at_23 = PolyComm::chunked_multi_scalar_mul_verifier(&comms, &scalars, Fp::from(23u64));
        let at_29 = PolyComm::chunked_multi_scalar_mul_verifier(&comms, &scalars, Fp::from(29u64));
        assert_ne!(at_23, at_29);
    }

    /// An empty input keeps `multi_scalar_mul`'s convention: one chunk holding
    /// the point at infinity, not an empty commitment.
    #[test]
    fn a_fused_chunked_msm_on_nothing_is_one_infinity_chunk() {
        use poly_commitment::commitment::PolyComm;

        let out = PolyComm::<Vesta>::chunked_multi_scalar_mul_verifier(&[], &[], Fp::from(5u64));
        assert_eq!(out.chunks.len(), 1);
        assert!(out.chunks[0].is_zero());
    }
}

/// `x <- x * y mod p` through the fixed-modulus Pasta syscall for the field
/// selected at compile time: `PALLAS_FP_MUL` (0x00_01_01_76) for Fp, or
/// `VESTA_FP_MUL` (0x00_01_01_79) for Fq. Only `x` and `y` are memory operands;
/// the modulus is a constant of the syscall's AIR.
///
/// # Safety
/// `x` must be a valid, aligned, writable `[u64; 4]` and `y` a valid, aligned
/// `[u64; 4]` that does not alias `x`: both are memory operands of one syscall.
#[cfg(target_os = "zkvm")]
#[inline(always)]
#[allow(unsafe_code)]
pub(crate) unsafe fn pasta_fp_mul<const FQ: bool>(x: *mut [u64; 4], y: *const [u64; 4]) {
    let code: u32 = if FQ { 0x00_01_01_79 } else { 0x00_01_01_76 };
    core::arch::asm!("ecall", in("t0") code, in("a0") x, in("a1") y);
}
