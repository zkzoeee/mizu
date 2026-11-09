// Compile the vendored arithmetic tests in a runnable workspace test target.
// The native library keeps the ordinary permutation, providing an independent
// reference for every canonical round and parameter fallback.
pub use mina_poseidon::{constants, pasta, poseidon};
#[path = "../../../vendor/proof-systems/poseidon/src/canonical.rs"]
mod canonical;

/// The deferred-reduction MDS row against the three-reduction row it may
/// replace, on edge values and a deterministic spread, in both Pasta fields.
#[test]
fn deferred_mds_row_matches_prepared_row() {
    use ark_ff::{BigInt, BigInteger};
    use mina_curves::pasta::canonical_words::{CanonicalPasta, CanonicalPastaRhs};

    fn check<const FQ: bool>() {
        let mut minus_one = CanonicalPasta::<FQ>::MODULUS;
        minus_one.sub_with_borrow(&BigInt::from(1u64));
        let edges = [[0u64; 4], [1, 0, 0, 0], [u64::MAX, 0, 0, 0], minus_one.0];
        let mut values: Vec<CanonicalPasta<FQ>> = edges
            .iter()
            .map(|l| CanonicalPasta::from_limbs(*l).unwrap())
            .collect();
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        while values.len() < 40 {
            let mut limbs = [0u64; 4];
            for limb in limbs.iter_mut() {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *limb = state;
            }
            limbs[3] &= (1 << 62) - 1;
            values.push(CanonicalPasta::from_limbs(limbs).unwrap());
        }
        for window in values.windows(6) {
            let a = [window[0], window[1], window[2]].map(CanonicalPastaRhs::<FQ>::new);
            let b = [window[3], window[4], window[5]];
            assert_eq!(
                CanonicalPasta::sum3_prepared_products_deferred(&a, &b),
                CanonicalPasta::sum3_prepared_products(&a, &b),
            );
        }
        // Every operand at p - 1 maximizes the unreduced sum.
        let max = CanonicalPasta::<FQ>::from_limbs(minus_one.0).unwrap();
        let a = [CanonicalPastaRhs::<FQ>::new(max); 3];
        assert_eq!(
            CanonicalPasta::sum3_prepared_products_deferred(&a, &[max; 3]),
            CanonicalPasta::sum3_prepared_products(&a, &[max; 3]),
        );
    }
    check::<false>();
    check::<true>();
}
