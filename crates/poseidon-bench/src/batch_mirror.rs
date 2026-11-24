//! Optimized multi-account mirror tree primitives:
//! - Opt 1: Sorted bottom-up Merkle multiproof (shared ancestors hashed once).
//! - Opt 2: Hot-account deduplication (collapse repeated touches to net old -> new).
//! - Opt 3: `keccak(H_p || evm_fields)` leaf binding (1 keccak perm instead of 3).
//! - Opt 5: Contiguous phase separation (all Poseidon, then all Keccak).

extern crate alloc;
use alloc::vec::Vec;

/// One raw account transition in a block or multi-block epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountDiff {
    pub leaf_idx: u64,
    pub seq: u32,
    pub old_seed: u64,
    pub new_seed: u64,
}

/// Net transition for a unique leaf index after deduplication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UniqueDiff {
    pub leaf_idx: u64,
    pub old_seed: u64,
    pub new_seed: u64,
}

/// Optimization 2: sort by `(leaf_idx, seq)` and collapse repeated touches to the
/// same `leaf_idx` into a single net transition `(first.old_seed -> last.new_seed)`.
pub fn dedup_diffs(diffs: &mut [AccountDiff]) -> Vec<UniqueDiff> {
    diffs.sort_unstable_by_key(|d| (d.leaf_idx, d.seq));
    let mut out = Vec::with_capacity(diffs.len());
    for d in diffs.iter() {
        if let Some(last) = out.last_mut() {
            let last: &mut UniqueDiff = last;
            if last.leaf_idx == d.leaf_idx {
                last.new_seed = d.new_seed;
                continue;
            }
        }
        out.push(UniqueDiff {
            leaf_idx: d.leaf_idx,
            old_seed: d.old_seed,
            new_seed: d.new_seed,
        });
    }
    out
}

/// Optimization 1: sorted bottom-up Merkle multiproof.
///
/// Given a `leaf_idx`-sorted slice of `(leaf_idx, old_leaf, new_leaf)` pairs,
/// computes `(old_root, new_root)` in a single bottom-up pass. Whenever two
/// adjacent frontier entries are siblings (`idx` even and `idx + 1`), they are
/// merged directly without consulting an external sibling, so every shared
/// ancestor is hashed once instead of once per updated leaf.
///
/// `sibling_fn(level, sibling_node_idx)` supplies the unchanged sibling hash
/// whenever a frontier node's sibling is not in the updated set.
///
/// Returns `(old_root, new_root, internal_node_hashes)` where `internal_node_hashes`
/// is the total number of `(old, new)` pair hash calls (2x `hash_node` calls).
pub fn batch_merkle_update<H: Copy>(
    mut frontier: Vec<(u64, H, H)>,
    depth: u32,
    mut hash_node: impl FnMut(&H, &H) -> H,
    mut sibling_fn: impl FnMut(u32, u64) -> H,
) -> (H, H, u64) {
    assert!(!frontier.is_empty(), "frontier must be non-empty");
    let mut next_frontier: Vec<(u64, H, H)> = Vec::with_capacity(frontier.len());
    let mut pairs_hashed = 0u64;

    for level in 0..depth {
        next_frontier.clear();
        let mut i = 0;
        while i < frontier.len() {
            let (idx, old_v, new_v) = frontier[i];
            let parent = idx >> 1;
            if (idx & 1 == 0) && (i + 1 < frontier.len()) && (frontier[i + 1].0 == idx + 1) {
                // Both children are in the updated set: merge directly!
                let (_, old_r, new_r) = frontier[i + 1];
                let old_p = hash_node(&old_v, &old_r);
                let new_p = hash_node(&new_v, &new_r);
                next_frontier.push((parent, old_p, new_p));
                pairs_hashed += 1;
                i += 2;
            } else {
                let sib = sibling_fn(level, idx ^ 1);
                let (old_p, new_p) = if idx & 1 == 0 {
                    (hash_node(&old_v, &sib), hash_node(&new_v, &sib))
                } else {
                    (hash_node(&sib, &old_v), hash_node(&sib, &new_v))
                };
                next_frontier.push((parent, old_p, new_p));
                pairs_hashed += 1;
                i += 1;
            }
        }
        core::mem::swap(&mut frontier, &mut next_frontier);
    }

    debug_assert_eq!(frontier.len(), 1);
    (frontier[0].1, frontier[0].2, pairs_hashed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use sha3::{Digest, Keccak256};

    fn hash_pair(l: &[u8; 32], r: &[u8; 32]) -> [u8; 32] {
        let mut h = Keccak256::new();
        h.update(l);
        h.update(r);
        h.finalize().into()
    }

    fn leaf_hash(seed: u64) -> [u8; 32] {
        let mut h = Keccak256::new();
        h.update(seed.to_le_bytes());
        h.finalize().into()
    }

    /// Reference dense binary Merkle tree of depth `depth` (`2^depth` leaves).
    fn build_levels(leaves: &[[u8; 32]], depth: u32) -> Vec<Vec<[u8; 32]>> {
        let mut levels = Vec::with_capacity(depth as usize + 1);
        levels.push(leaves.to_vec());
        for _ in 0..depth {
            let prev = levels.last().unwrap();
            let mut next = Vec::with_capacity(prev.len() / 2);
            for pair in prev.chunks_exact(2) {
                next.push(hash_pair(&pair[0], &pair[1]));
            }
            levels.push(next);
        }
        levels
    }

    #[test]
    fn batch_multiproof_matches_dense_merkle_tree_and_saves_hashes() {
        let depth = 8u32;
        let n_leaves = 1usize << depth;
        let old_leaves: Vec<[u8; 32]> = (0..n_leaves as u64).map(leaf_hash).collect();
        let old_levels = build_levels(&old_leaves, depth);
        let expected_old_root = old_levels[depth as usize][0];

        // Mix of clustered, scattered, and duplicate (hot-account) updates.
        let mut raw = vec![
            AccountDiff { leaf_idx: 5, seq: 0, old_seed: 5, new_seed: 1005 },
            AccountDiff { leaf_idx: 4, seq: 1, old_seed: 4, new_seed: 1004 },
            AccountDiff { leaf_idx: 5, seq: 2, old_seed: 1005, new_seed: 2005 }, // hot account!
            AccountDiff { leaf_idx: 200, seq: 3, old_seed: 200, new_seed: 1200 },
            AccountDiff { leaf_idx: 201, seq: 4, old_seed: 201, new_seed: 1201 },
            AccountDiff { leaf_idx: 202, seq: 5, old_seed: 202, new_seed: 1202 },
            AccountDiff { leaf_idx: 203, seq: 6, old_seed: 203, new_seed: 1203 },
            AccountDiff { leaf_idx: 127, seq: 7, old_seed: 127, new_seed: 1127 },
            AccountDiff { leaf_idx: 128, seq: 8, old_seed: 128, new_seed: 1128 },
            AccountDiff { leaf_idx: 200, seq: 9, old_seed: 1200, new_seed: 3200 }, // hot account!
        ];

        // Apply sequentially to dense reference tree.
        let mut new_leaves = old_leaves.clone();
        for d in &raw {
            assert_eq!(new_leaves[d.leaf_idx as usize], leaf_hash(d.old_seed));
            new_leaves[d.leaf_idx as usize] = leaf_hash(d.new_seed);
        }
        let expected_new_root = build_levels(&new_leaves, depth)[depth as usize][0];

        // Deduplicate + run bottom-up multiproof.
        let unique = dedup_diffs(&mut raw);
        assert_eq!(unique.len(), 8); // 10 raw touches collapsed to 8 unique accounts
        let frontier: Vec<(u64, [u8; 32], [u8; 32])> = unique
            .iter()
            .map(|u| (u.leaf_idx, leaf_hash(u.old_seed), leaf_hash(u.new_seed)))
            .collect();

        let (got_old, got_new, pairs_hashed) = batch_merkle_update(
            frontier,
            depth,
            hash_pair,
            |level, sib_idx| old_levels[level as usize][sib_idx as usize],
        );

        assert_eq!(got_old, expected_old_root, "old root mismatch");
        assert_eq!(got_new, expected_new_root, "new root mismatch");

        // Naive sequential would hash 10 * 8 = 80 pairs; dedup + multiproof must be strictly less!
        assert!(
            pairs_hashed < 40,
            "expected substantial multiproof savings, got {pairs_hashed} pairs (naive 80)"
        );
    }
}
