//! Seeded differential fuzz for the closed canonical Pasta residues.
//!
//! `CanonicalPasta` exists for the zkVM, where its products become syscalls.
//! On the host, `mul_assign` delegates to arkworks, so what these cases
//! actually compare is everything built around the product: the conditional
//! subtractions in `add_assign`, the multiplication schedules in `pow7` and
//! `pow7_prepared`, the wide accumulation and up-to-two reductions in
//! `sum3_products` and `sum3_prepared_products`, and the 512-bit accumulator,
//! `2^256 mod p` folding and up-to-three reductions in
//! `sum3_prepared_products_deferred`. Those bounds are argued in comments; the
//! cases below put them to random and boundary inputs instead.
//!
//! The existing tests for this module are fixed vectors. The stream here is a
//! dependency-free splitmix64 seeded by a constant, so a failure replays.
//!
//! It lives here rather than in the fork because the fork's own workspace
//! needs the network to resolve a git dependency, while this crate's tests
//! already reach into the fork's internals and run offline.

use ark_ff::{BigInt, BigInteger, PrimeField};
use mina_curves::pasta::canonical_words::{CanonicalPasta, CanonicalPastaRhs};
use mina_curves::pasta::{Fp, Fq};

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

    fn bytes32(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for chunk in out.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        out
    }
}

fn limbs_of<F: PrimeField>(value: F) -> [u64; 4] {
    value.into_bigint().as_ref().try_into().expect("Pasta elements are four limbs")
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

/// The boundary classes, before the random ones: zero, one, two, the halves
/// around `p / 2`, and the largest residues.
fn boundaries<F: PrimeField>() -> Vec<F> {
    let p_minus_1 = -F::one();
    vec![
        F::zero(),
        F::one(),
        F::from(2u64),
        F::from(3u64),
        p_minus_1,
        p_minus_1 - F::one(),
        p_minus_1 / F::from(2u64),
        (p_minus_1 / F::from(2u64)) + F::one(),
        F::from_le_bytes_mod_order(&[0xff; 32]),
    ]
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
    assert!(CanonicalPasta::<FQ>::from_limbs(above.0).is_none(), "p + 1 is not canonical");
    assert!(
        CanonicalPasta::<FQ>::from_limbs([u64::MAX; 4]).is_none(),
        "2^256 - 1 is not canonical"
    );
    let mut below = modulus;
    below.sub_with_borrow(&BigInt::<4>::from(1u64));
    let largest = CanonicalPasta::<FQ>::from_limbs(below.0).expect("p - 1 is canonical");
    assert_eq!(largest.limbs(), below.0, "limbs must round-trip");

    // Operands: every ordered pair of boundary classes, then random triples.
    let boundary = boundaries::<F>();
    let mut pairs: Vec<(F, F)> = Vec::new();
    for a in &boundary {
        for b in &boundary {
            pairs.push((*a, *b));
        }
    }
    for _ in 0..ITERATIONS {
        pairs.push((
            F::from_le_bytes_mod_order(&rng.bytes32()),
            F::from_le_bytes_mod_order(&rng.bytes32()),
        ));
    }

    for (a, b) in pairs {
        let (ca, cb) = (canonical::<FQ, F>(a), canonical::<FQ, F>(b));

        let mut sum = ca;
        sum.add_assign(&cb);
        assert_eq!(sum.limbs(), limbs_of(a + b), "add_assign disagrees for {a} + {b}");
        // Addition must stay closed, which is what the conditional subtraction
        // is for: the sum of two residues is below 2p and above p half the time.
        assert_eq!(field_of::<F>(sum.limbs()), a + b);

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

    // The three-term inner products, which carry the bound arguments worth
    // testing: one reduction deferred across three unreduced products.
    let mut triples: Vec<([F; 3], [F; 3])> = vec![
        ([F::zero(); 3], [F::zero(); 3]),
        ([-F::one(); 3], [-F::one(); 3]),
        ([-F::one(), F::zero(), F::one()], [-F::one(), -F::one(), -F::one()]),
        ([F::one(); 3], [-F::one(); 3]),
    ];
    for _ in 0..ITERATIONS {
        triples.push((
            core::array::from_fn(|_| F::from_le_bytes_mod_order(&rng.bytes32())),
            core::array::from_fn(|_| F::from_le_bytes_mod_order(&rng.bytes32())),
        ));
    }
    for (a, b) in triples {
        let expected = limbs_of(a[0] * b[0] + a[1] * b[1] + a[2] * b[2]);
        let ca: [CanonicalPasta<FQ>; 3] = core::array::from_fn(|i| canonical::<FQ, F>(a[i]));
        let cb: [CanonicalPasta<FQ>; 3] = core::array::from_fn(|i| canonical::<FQ, F>(b[i]));
        let rhs: [CanonicalPastaRhs<FQ>; 3] = core::array::from_fn(|i| CanonicalPastaRhs::new(ca[i]));
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
