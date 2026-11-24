//! Seeded differential fuzz for canonical Pasta arithmetic, point/MSM kernels,
//! GLV decomposition, and untrusted wire/blob parsers.
//!
//! Uses a dependency-free `splitmix64` stream with fixed seeds so any failure
//! replays deterministically offline.

use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::{AdditiveGroup, BigInt, BigInteger, Field, PrimeField, Zero};
use mina_curves::pasta::{
    canonical_pallas,
    canonical_words::{CanonicalPasta, CanonicalPastaRhs},
    pallas_glv,
    point_spikes::{self, PastaCurve},
    Fp, Fq, Pallas, Vesta, VestaParameters,
};
use pickles_verifier::wire::{
    parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof,
};

const ITERATIONS: usize = 512;

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }

    fn bytes32(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for chunk in out.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        out
    }

    fn field<F: PrimeField>(&mut self) -> F {
        F::from_le_bytes_mod_order(&self.bytes32())
    }
}

fn limbs_of<F: PrimeField>(value: F) -> [u64; 4] {
    value
        .into_bigint()
        .as_ref()
        .try_into()
        .expect("Pasta elements are four limbs")
}

fn field_of<F: PrimeField>(limbs: [u64; 4]) -> F {
    let mut bytes = [0u8; 32];
    for (chunk, limb) in bytes.chunks_mut(8).zip(limbs) {
        chunk.copy_from_slice(&limb.to_le_bytes());
    }
    F::from_le_bytes_mod_order(&bytes)
}

fn canonical<const FQ: bool, F: PrimeField>(value: F) -> CanonicalPasta<FQ> {
    CanonicalPasta::from_limbs(limbs_of(value)).expect("a field element is canonical")
}

/// Boundary classes: zero, small integers, halves around `p / 2`, window-edge
/// powers (`2^120`, `2^126`, `2^128`, `2^130`, `2^240`, `2^252`), and `p - 1`.
fn boundaries<F: PrimeField>() -> Vec<F> {
    let p_minus_1 = -F::one();
    let mut out = vec![
        F::zero(),
        F::one(),
        F::from(2u64),
        F::from(3u64),
        p_minus_1,
        p_minus_1 - F::one(),
        p_minus_1 / F::from(2u64),
        (p_minus_1 / F::from(2u64)) + F::one(),
        F::from_le_bytes_mod_order(&[0xff; 32]),
    ];
    for shift in [120u32, 126, 127, 128, 130, 240, 252] {
        let mut bytes = [0u8; 32];
        bytes[(shift / 8) as usize] = 1u8 << (shift % 8);
        let pow = F::from_le_bytes_mod_order(&bytes);
        out.push(pow);
        out.push(pow - F::one());
        out.push(pow + F::one());
    }
    out
}

fn check_field<const FQ: bool, F: PrimeField>(seed: u64) {
    let mut rng = Rng(seed);
    let modulus = BigInt::<4>(CanonicalPasta::<FQ>::MODULUS.0);

    // `from_limbs` is the only constructor, and it is what keeps every
    // operation below inside the field.
    assert!(
        CanonicalPasta::<FQ>::from_limbs(modulus.0).is_none(),
        "the modulus itself is not a canonical residue"
    );
    let mut above = modulus;
    above.add_with_carry(&BigInt::<4>::from(1u64));
    assert!(
        CanonicalPasta::<FQ>::from_limbs(above.0).is_none(),
        "p + 1 is not canonical"
    );
    assert!(
        CanonicalPasta::<FQ>::from_limbs([u64::MAX; 4]).is_none(),
        "2^256 - 1 is not canonical"
    );
    let mut below = modulus;
    below.sub_with_borrow(&BigInt::<4>::from(1u64));
    let largest = CanonicalPasta::<FQ>::from_limbs(below.0).expect("p - 1 is canonical");
    assert_eq!(largest.limbs(), below.0, "limbs must round-trip");
    assert!(
        CanonicalPasta::<FQ>::ZERO.inverse().is_none(),
        "zero must have no inverse"
    );

    // Operands: every ordered pair of boundary classes, then random pairs.
    let boundary = boundaries::<F>();
    let mut pairs: Vec<(F, F)> = Vec::new();
    for a in &boundary {
        for b in &boundary {
            pairs.push((*a, *b));
        }
    }
    for _ in 0..ITERATIONS {
        pairs.push((rng.field::<F>(), rng.field::<F>()));
    }

    for (a, b) in pairs {
        let (ca, cb) = (canonical::<FQ, F>(a), canonical::<FQ, F>(b));

        let mut sum = ca;
        sum.add_assign(&cb);
        assert_eq!(sum.limbs(), limbs_of(a + b), "add_assign disagrees for {a} + {b}");
        assert_eq!(field_of::<F>(sum.limbs()), a + b);

        let mut diff = ca;
        diff.sub_assign(&cb);
        assert_eq!(diff.limbs(), limbs_of(a - b), "sub_assign disagrees for {a} - {b}");
        assert_eq!(field_of::<F>(diff.limbs()), a - b);

        let mut product = ca;
        product.mul_assign(&cb);
        assert_eq!(product.limbs(), limbs_of(a * b), "mul_assign disagrees for {a} * {b}");

        let mut prepared = ca;
        prepared.mul_prepared_assign(&CanonicalPastaRhs::new(cb));
        assert_eq!(prepared.limbs(), limbs_of(a * b), "mul_prepared_assign disagrees");

        let seventh = a.pow([7u64]);
        assert_eq!(ca.pow7().limbs(), limbs_of(seventh), "pow7 disagrees for {a}");
        assert_eq!(
            ca.pow7_prepared().limbs(),
            limbs_of(seventh),
            "pow7_prepared disagrees for {a}"
        );
    }

    // Inversion over all boundaries + random elements.
    for a in boundary.into_iter().chain((0..ITERATIONS).map(|_| rng.field::<F>())) {
        let ca = canonical::<FQ, F>(a);
        match (a.inverse(), ca.inverse()) {
            (None, None) => {}
            (Some(exp), Some(got)) => {
                assert_eq!(got.limbs(), limbs_of(exp), "inverse disagrees for {a}");
                let mut prod = ca;
                prod.mul_assign(&got);
                assert_eq!(prod.limbs(), [1, 0, 0, 0], "a * a^-1 != 1");
            }
            (exp, got) => panic!("inverse presence mismatch for {a}: {exp:?} vs {got:?}"),
        }
    }

    // Three-term inner products (deferred reduction bounds).
    let mut triples: Vec<([F; 3], [F; 3])> = vec![
        ([F::zero(); 3], [F::zero(); 3]),
        ([-F::one(); 3], [-F::one(); 3]),
        ([-F::one(), F::zero(), F::one()], [-F::one(), -F::one(), -F::one()]),
        ([F::one(); 3], [-F::one(); 3]),
    ];
    for _ in 0..ITERATIONS {
        triples.push((
            core::array::from_fn(|_| rng.field::<F>()),
            core::array::from_fn(|_| rng.field::<F>()),
        ));
    }
    for (a, b) in triples {
        let expected = limbs_of(a[0] * b[0] + a[1] * b[1] + a[2] * b[2]);
        let ca: [CanonicalPasta<FQ>; 3] = core::array::from_fn(|i| canonical::<FQ, F>(a[i]));
        let cb: [CanonicalPasta<FQ>; 3] = core::array::from_fn(|i| canonical::<FQ, F>(b[i]));
        let rhs: [CanonicalPastaRhs<FQ>; 3] =
            core::array::from_fn(|i| CanonicalPastaRhs::new(ca[i]));
        assert_eq!(
            CanonicalPasta::sum3_products(&ca, &cb).limbs(),
            expected,
            "sum3_products disagrees"
        );
        assert_eq!(
            CanonicalPasta::sum3_prepared_products(&rhs, &cb).limbs(),
            expected,
            "sum3_prepared_products disagrees"
        );
        assert_eq!(
            CanonicalPasta::sum3_prepared_products_deferred(&rhs, &cb).limbs(),
            expected,
            "sum3_prepared_products_deferred disagrees"
        );
    }
}

#[test]
fn canonical_fp_matches_arkworks() {
    check_field::<false, Fp>(0x5eed_0f90_0001);
}

#[test]
fn canonical_fq_matches_arkworks() {
    check_field::<true, Fq>(0x5eed_0f90_0002);
}

#[test]
fn point_ops_and_software_double_match_arkworks() {
    let mut rng = Rng(0x5eed_0f90_0003);
    let vg = Vesta::generator();
    let pg = Pallas::generator();

    assert_eq!(
        point_spikes::double_software(&Vesta::zero(), PastaCurve::Vesta),
        Vesta::zero()
    );
    assert_eq!(
        point_spikes::double_software(&Pallas::zero(), PastaCurve::Pallas),
        Pallas::zero()
    );

    for _ in 0..128 {
        let p = (vg * rng.field::<Fp>()).into_affine();
        let q = (vg * rng.field::<Fp>()).into_affine();
        assert_eq!(
            point_spikes::double_software(&p, PastaCurve::Vesta),
            (p + p).into_affine()
        );
        assert_eq!(
            point_spikes::double(&p, PastaCurve::Vesta),
            (p + p).into_affine()
        );
        assert_eq!(
            point_spikes::add(&p, &q, PastaCurve::Vesta),
            (p + q).into_affine()
        );
        assert_eq!(
            point_spikes::add(&p, &p, PastaCurve::Vesta),
            (p + p).into_affine()
        );
        assert_eq!(
            point_spikes::add(&p, &(-p), PastaCurve::Vesta),
            Vesta::zero()
        );

        let a = (pg * rng.field::<Fq>()).into_affine();
        let b = (pg * rng.field::<Fq>()).into_affine();
        assert_eq!(
            point_spikes::double_software(&a, PastaCurve::Pallas),
            (a + a).into_affine()
        );
        assert_eq!(
            point_spikes::double(&a, PastaCurve::Pallas),
            (a + a).into_affine()
        );
        assert_eq!(
            point_spikes::add(&a, &b, PastaCurve::Pallas),
            (a + b).into_affine()
        );
        assert_eq!(
            point_spikes::add(&a, &a, PastaCurve::Pallas),
            (a + a).into_affine()
        );
        assert_eq!(
            point_spikes::add(&a, &(-a), PastaCurve::Pallas),
            Pallas::zero()
        );
    }
}

#[test]
fn pallas_glv_decompose_fuzz_matches_scalar_mul() {
    let mut rng = Rng(0x5eed_0f90_0004);
    let beta = pallas_glv::endomorphism_coefficient();
    let g = Pallas::generator();

    let mut scalars = boundaries::<Fq>();
    for _ in 0..ITERATIONS {
        scalars.push(rng.field::<Fq>());
    }
    for s in scalars {
        let p = (g * rng.field::<Fq>()).into_affine();
        if p.is_zero() {
            continue;
        }
        let ((neg1, k1), (neg2, k2)) = pallas_glv::decompose_bigint(s.into_bigint());
        let mut phi = p;
        phi.x *= beta;
        let term1 = {
            let t = p.mul_bigint(BigInt([k1 as u64, (k1 >> 64) as u64, 0, 0]));
            if neg1 { -t } else { t }
        };
        let term2 = {
            let t = phi.mul_bigint(BigInt([k2 as u64, (k2 >> 64) as u64, 0, 0]));
            if neg2 { -t } else { t }
        };
        assert_eq!(
            (term1 + term2).into_affine(),
            p.mul_bigint(s.into_bigint()).into_affine(),
            "Pallas GLV decomposition mismatch for scalar {s}"
        );
    }
}

fn build_finite_planes_vesta(bases: &[Vesta], lane_shift: usize) -> Vec<[u64; 8]> {
    let mut out = Vec::with_capacity(bases.len() * 2);
    for p in bases {
        let x = p.x.into_bigint().0;
        let y = p.y.into_bigint().0;
        out.push([x[0], x[1], x[2], x[3], y[0], y[1], y[2], y[3]]);
    }
    for p in bases {
        let mut q = p.into_group();
        for _ in 0..lane_shift {
            q.double_in_place();
        }
        let qa = q.into_affine();
        let x = qa.x.into_bigint().0;
        let y = qa.y.into_bigint().0;
        out.push([x[0], x[1], x[2], x[3], y[0], y[1], y[2], y[3]]);
    }
    out
}

fn build_finite_planes_pallas(bases: &[Pallas], lane_shift: usize) -> Vec<[u64; 8]> {
    let mut out = Vec::with_capacity(bases.len() * 2);
    for p in bases {
        let x = p.x.into_bigint().0;
        let y = p.y.into_bigint().0;
        out.push([x[0], x[1], x[2], x[3], y[0], y[1], y[2], y[3]]);
    }
    for p in bases {
        let mut q = p.into_group();
        for _ in 0..lane_shift {
            q.double_in_place();
        }
        let qa = q.into_affine();
        let x = qa.x.into_bigint().0;
        let y = qa.y.into_bigint().0;
        out.push([x[0], x[1], x[2], x[3], y[0], y[1], y[2], y[3]]);
    }
    out
}

#[test]
fn coupled_finite_vesta_and_pallas_msms_fuzz() {
    let mut rng = Rng(0x5eed_0f90_0005);
    let vg = Vesta::generator();
    let pg = Pallas::generator();

    // Vesta: include duplicate bases and negated bases to stress equal-x / cancellation paths.
    let v_bases: Vec<Vesta> = (0..32)
        .map(|i| match i {
            3 => (vg * Fp::from(7u64)).into_affine(),
            4 => (vg * Fp::from(7u64)).into_affine(),
            5 => -(vg * Fp::from(7u64)).into_affine(),
            _ => {
                let mut s = rng.field::<Fp>();
                if s.is_zero() {
                    s = Fp::ONE;
                }
                (vg * s).into_affine()
            }
        })
        .collect();
    let v_tables = [
        (16usize, 128usize, build_finite_planes_vesta(&v_bases, 128)),
        (15, 120, build_finite_planes_vesta(&v_bases, 120)),
        (14, 126, build_finite_planes_vesta(&v_bases, 126)),
    ];
    let v_bounds = boundaries::<Fp>();
    for trial in 0..24 {
        let scalars: Vec<Fp> = (0..v_bases.len())
            .map(|i| {
                if trial == 0 {
                    v_bounds[i % v_bounds.len()]
                } else if trial == 1 {
                    Fp::ZERO
                } else if i % 4 == 0 {
                    v_bounds[rng.below(v_bounds.len())]
                } else {
                    rng.field::<Fp>()
                }
            })
            .collect();
        let expected = v_bases
            .iter()
            .zip(&scalars)
            .fold(<Vesta as AffineRepr>::Group::zero(), |acc, (b, s)| {
                acc + b.mul_bigint(s.into_bigint())
            });
        let canonical_s: Vec<_> = scalars
            .iter()
            .map(|s| CanonicalPasta::<false>::from_limbs(s.into_bigint().0).unwrap())
            .collect();
        for (width, shift, coords) in &v_tables {
            let actual = point_spikes::msm_vesta_finite_borrowed::<VestaParameters>(
                coords,
                &canonical_s,
                *width,
                *shift,
            );
            assert_eq!(
                actual, expected,
                "Vesta finite borrowed MSM mismatch (trial={trial}, w={width}, shift={shift})"
            );
        }
    }

    // Pallas: coupled (shift=130, w=13) and uncoupled (shift=128, w=13) with
    // mixed tail containing duplicates, opposites, and infinity points.
    let p_bases: Vec<Pallas> = (0..24)
        .map(|i| match i {
            2 => (pg * Fq::from(19u64)).into_affine(),
            3 => -(pg * Fq::from(19u64)).into_affine(),
            _ => {
                let mut s = rng.field::<Fq>();
                if s.is_zero() {
                    s = Fq::ONE;
                }
                (pg * s).into_affine()
            }
        })
        .collect();
    let p_tables = [
        (128usize, build_finite_planes_pallas(&p_bases, 128)),
        (130usize, build_finite_planes_pallas(&p_bases, 130)),
    ];
    let p_bounds = boundaries::<Fq>();
    for trial in 0..24 {
        let prefix_len = if trial == 1 {
            0
        } else {
            1 + rng.below(p_bases.len())
        };
        let prefix_fq: Vec<Fq> = (0..prefix_len)
            .map(|i| {
                if trial == 0 {
                    p_bounds[i % p_bounds.len()]
                } else if i % 3 == 0 {
                    p_bounds[rng.below(p_bounds.len())]
                } else {
                    rng.field::<Fq>()
                }
            })
            .collect();
        let prefix_ks: Vec<BigInt<4>> = prefix_fq.iter().map(|s| s.into_bigint()).collect();
        let h = (pg * Fq::from(31u64)).into_affine();
        let hk = p_bounds[rng.below(p_bounds.len())].into_bigint();
        let t0 = (pg * Fq::from(101u64)).into_affine();
        let tail = [t0, Pallas::zero(), -t0, t0];
        let tk: [BigInt<4>; 4] = core::array::from_fn(|_| rng.field::<Fq>().into_bigint());

        let mut expected = h.mul_bigint(hk);
        for (p, k) in p_bases[..prefix_len].iter().zip(&prefix_ks) {
            expected += p.mul_bigint(*k);
        }
        for (p, k) in tail.iter().zip(&tk) {
            expected += p.mul_bigint(*k);
        }
        for (shift, coords) in &p_tables {
            let actual = canonical_pallas::msm_bigint_fixed_prefix_coords(
                coords,
                *shift,
                &prefix_ks,
                &h,
                hk,
                &tail,
                &tk,
                Some(13),
            );
            assert_eq!(
                actual, expected,
                "Pallas finite prefix MSM mismatch (trial={trial}, shift={shift})"
            );
        }
    }
}

#[test]
fn wire_parsers_reject_malformed_inputs_without_panicking() {
    let mut rng = Rng(0x5eed_0f90_0006);
    let edge_cases = [
        "",
        "{}",
        "[]",
        "null",
        "{\"statement\":null}",
        "[\"0x\"]",
        "[\"0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff00\"]",
        "[[1,2,3,4,5]]",
        "{\"evals\":[]}",
    ];
    for input in edge_cases {
        let _ = parse_wrap_vk(input);
        let _ = parse_wrap_proof(input);
        let _ = OcamlProof::parse(input);
        let _ = parse_app_statement_fields(input);
    }

    // Seeded pseudo-random ASCII/JSON-like fuzz corpus: must never panic.
    let alphabet = b"{}[]\":,0123456789abcdefxtruefalsenull- \n\t\\";
    for _ in 0..256 {
        let len = 1 + rng.below(192);
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            s.push(alphabet[rng.below(alphabet.len())] as char);
        }
        let _ = parse_wrap_vk(&s);
        let _ = parse_wrap_proof(&s);
        let _ = OcamlProof::parse(&s);
        let _ = parse_app_statement_fields(&s);
    }
}
