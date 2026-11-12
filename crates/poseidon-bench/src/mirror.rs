//! Mirror-root operation benchmark: what a keccak mirror of the Poseidon
//! ledger costs in SP1 guest cycles, measured rather than extrapolated.
//!
//! The diff-proof shape this measures, for each account a batch touched:
//!
//!   assert poseidon_root(idx, H_p(old), path_p) == p
//!   assert keccak_root  (idx, H_k(old), path_k) == k      <- the mirror
//!   p = poseidon_root(idx, H_p(new), path_p)
//!   k = keccak_root  (idx, H_k(new), path_k)              <- the mirror
//!
//! so every account costs TWO path recomputations per tree: one to check the
//! old root, one to produce the new one. That factor of two is why the mirror
//! is worth measuring instead of guessing.
//!
//! WHAT THIS IS NOT. It counts permutations and cycles for the hash schedule
//! of each mirror construction. It is not Mina's exact ledger hash: Mina's
//! node hash is `hash_with_prefix(MinaMklTree{height}, [left, right])`, whose
//! per-height prefix is absorbed into a precomputed initial state, so it costs
//! ONE permutation per node -- the same count this measures -- but not the same
//! bytes. No root here is claimed to equal a real Zeko root. Correctness of the
//! schedule is checked by the runner recomputing it natively.
//!
//! Keccak is the `sha3` software implementation, already a pinned workspace
//! dependency. SP1 ships a KECCAK_PERMUTE precompile and the fork vendors
//! `tiny-keccak-patched`; routing to it would make the mirror cheaper than
//! anything measured here, so every keccak figure below is an upper bound.
#![no_main]
sp1_zkvm::entrypoint!(main);
#[cfg(target_os = "zkvm")]
#[path = "../../o1-verifier/src/atomic_shims.rs"]
mod atomic_shims;

use ark_ff::{AdditiveGroup, PrimeField};
use mina_curves::pasta::Fp;
use mina_poseidon::{
    constants::PlonkSpongeConstantsKimchi, pasta::fp_kimchi, permutation::poseidon_block_cipher,
    poseidon::ArithmeticSpongeParams,
};
use sha3::{Digest, Keccak256};

/// Field elements in one account record, hashed at sponge rate 2.
const ACCOUNT_FIELDS: u32 = 16;
/// Bytes in one serialized account record. floor(b/136)+1 keccak permutations.
const ACCOUNT_BYTES: usize = 320;
/// Depth of the bucketed mirror in mode 2: the mirror covers only the top
/// levels, its leaves being Poseidon subtree roots.
const BUCKET_DEPTH: u32 = 16;

// ---------------------------------------------------------------- poseidon

/// One Merkle node: two children in, one field out, one permutation.
fn poseidon_node(params: &ArithmeticSpongeParams<Fp, 55>, left: Fp, right: Fp) -> Fp {
    let mut state = [left, right, Fp::ZERO];
    poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi, 55>(params, &mut state);
    state[0]
}

/// Account record -> one field. ceil(ACCOUNT_FIELDS/2) permutations at rate 2.
fn poseidon_leaf(params: &ArithmeticSpongeParams<Fp, 55>, seed: u64) -> Fp {
    let mut state = [Fp::ZERO, Fp::ZERO, Fp::ZERO];
    let mut i = 0u32;
    while i < ACCOUNT_FIELDS {
        state[0] += Fp::from(seed.wrapping_add(i as u64));
        state[1] += Fp::from(seed.wrapping_mul(i as u64 + 1));
        poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi, 55>(params, &mut state);
        i += 2;
    }
    state[0]
}

/// Walk one authentication path. `depth` permutations.
fn poseidon_path(
    params: &ArithmeticSpongeParams<Fp, 55>,
    mut acc: Fp,
    idx: u64,
    depth: u32,
    sibling: Fp,
) -> Fp {
    for level in 0..depth {
        acc = if (idx >> level) & 1 == 0 {
            poseidon_node(params, acc, sibling)
        } else {
            poseidon_node(params, sibling, acc)
        };
    }
    acc
}

// ------------------------------------------------------------------ keccak

/// One mirror node: 64 bytes in, 32 out. One keccak permutation.
fn keccak_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(left);
    h.update(right);
    h.finalize().into()
}

/// Account record -> 32 bytes. floor(ACCOUNT_BYTES/136)+1 permutations.
fn keccak_leaf(seed: u64) -> [u8; 32] {
    let mut h = Keccak256::new();
    let word = seed.to_le_bytes();
    let mut written = 0;
    while written < ACCOUNT_BYTES {
        h.update(word);
        written += word.len();
    }
    h.finalize().into()
}

fn keccak_path(mut acc: [u8; 32], idx: u64, depth: u32, sibling: &[u8; 32]) -> [u8; 32] {
    for level in 0..depth {
        acc = if (idx >> level) & 1 == 0 {
            keccak_node(&acc, sibling)
        } else {
            keccak_node(sibling, &acc)
        };
    }
    acc
}

// ------------------------------------------------------------------- modes

// The runner reads these markers out of the executor's cycle tracker. Written
// as literals, like the Poseidon operation guest, so the guest allocates
// nothing to report its own stages.
const POSEIDON_START: &[u8] = b"cycle-tracker-report-start:poseidon-ledger\n";
const POSEIDON_END: &[u8] = b"cycle-tracker-report-end:poseidon-ledger\n";
const MIRROR_START: &[u8] = b"cycle-tracker-report-start:keccak-mirror\n";
const MIRROR_END: &[u8] = b"cycle-tracker-report-end:keccak-mirror\n";

pub fn main() {
    // One read, not three: the runner hands the executor a single input
    // buffer, and a read consumes the whole of it.
    let (mode, accounts, depth) = sp1_zkvm::io::read::<(u32, u32, u32)>();
    assert!(mode <= 4, "mode out of range");
    assert!((1..=256).contains(&accounts), "accounts out of range");
    assert!((1..=40).contains(&depth), "depth out of range");

    sp1_zkvm::io::commit_slice(&mode.to_le_bytes());
    sp1_zkvm::io::commit_slice(&accounts.to_le_bytes());
    sp1_zkvm::io::commit_slice(&depth.to_le_bytes());

    let params = fp_kimchi::static_params();

    // Poseidon ledger: every mode pays this. It is the half we already have.
    sp1_zkvm::io::write(1, POSEIDON_START);
    let mut p = Fp::from(7u64);
    for account in 0..accounts as u64 {
        let idx = account.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let sibling = Fp::from(account.wrapping_add(11));
        let old_leaf = poseidon_leaf(params, account);
        let _check = poseidon_path(params, old_leaf, idx, depth, sibling);
        let new_leaf = poseidon_leaf(params, account.wrapping_add(1 << 20));
        p = poseidon_path(params, new_leaf, idx, depth, sibling);
    }
    sp1_zkvm::io::write(1, POSEIDON_END);
    for limb in p.into_bigint().as_ref() {
        sp1_zkvm::io::commit_slice(&limb.to_le_bytes());
    }

    // The mirror, in whichever construction this mode selects.
    sp1_zkvm::io::write(1, MIRROR_START);
    let mut k = [0u8; 32];
    match mode {
        // 0: no mirror. The baseline the other modes are read against.
        0 => {}
        // 1: full keccak Merkle mirror, leaf for leaf, same depth.
        1 => {
            for account in 0..accounts as u64 {
                let idx = account.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let sibling = keccak_leaf(account.wrapping_add(99));
                let old_leaf = keccak_leaf(account);
                let _check = keccak_path(old_leaf, idx, depth, &sibling);
                let new_leaf = keccak_leaf(account.wrapping_add(1 << 20));
                k = keccak_path(new_leaf, idx, depth, &sibling);
            }
        }
        // 2: bucketed mirror. Keccak covers the top BUCKET_DEPTH levels only;
        //    its leaves are Poseidon subtree roots, so Ethereum can still prove
        //    inclusion down to a bucket, not down to an account.
        2 => {
            let levels = core::cmp::min(BUCKET_DEPTH, depth);
            for account in 0..accounts as u64 {
                let idx = account.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let sibling = keccak_leaf(account.wrapping_add(99));
                let old_leaf = keccak_leaf(account);
                let _check = keccak_path(old_leaf, idx, levels, &sibling);
                let new_leaf = keccak_leaf(account.wrapping_add(1 << 20));
                k = keccak_path(new_leaf, idx, levels, &sibling);
            }
        }
        // 3: hash chain. k = keccak(k || idx || H_k(old) || H_k(new)) per
        //    account. No path, no tree: continuity only, no inclusion proofs.
        3 => {
            for account in 0..accounts as u64 {
                let idx = account.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let mut h = Keccak256::new();
                h.update(k);
                h.update(idx.to_le_bytes());
                h.update(keccak_leaf(account));
                h.update(keccak_leaf(account.wrapping_add(1 << 20)));
                k = h.finalize().into();
            }
        }
        // 4: one keccak over the whole sorted diff list. Cheapest possible
        //    mirror: the permutation count is the byte length of the diff set,
        //    not a function of depth at all.
        4 => {
            let mut h = Keccak256::new();
            for account in 0..accounts as u64 {
                let idx = account.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                h.update(idx.to_le_bytes());
                h.update(keccak_leaf(account));
                h.update(keccak_leaf(account.wrapping_add(1 << 20)));
            }
            k = h.finalize().into();
        }
        _ => unreachable!(),
    }
    sp1_zkvm::io::write(1, MIRROR_END);
    sp1_zkvm::io::commit_slice(&k);
}
