//! Exact Mina domain-separated Poseidon ledger hashing + dual-root Keccak mirror
//! (`keccakGlobalRoot` + `keccakDiffRoot`) for EVM settlement.
//!
//! Combines:
//! - Mina's exact 20-byte `*`-padded Poseidon prefixes:
//!   - `"MinaMklTree{:03}******"` (heights `0..depth`, precomputed into initial sponge states so
//!     every internal node costs **1 Poseidon permutation**).
//!   - `"MinaAccount*********"` (account leaf hash, rate 2).
//!   - `"MinaProtoState******"` (anchors `state_hash == proof.app_state[0]` when protocol-state
//!     binding is supplied).
//! - Opt 1: Sorted bottom-up Merkle multiproof (shared internal nodes hashed once).
//! - Opt 2: Hot-account deduplication (repeated touches collapsed to net `first_old -> last_new`).
//! - Opt 3: `keccak256(H_p || evm_fields)` bound leaves (112 bytes = **1 Keccak permutation**).
//! - Opt 5: Contiguous Poseidon -> Keccak phases (`tiny-keccak` SP1 precompile under
//!   `keccak-precompile`, zero wrap-VK / ceremony impact).
//! - Dual-root commitment: both `keccakGlobalRoot` (depth-35 global mirror, ~50k gas) and
//!   `keccakDiffRoot` (depth `ceil(log2 N)` compact diff root, ~25k gas, works directly with any
//!   unmodified `mina-indexer` GraphQL API).

use alloc::vec::Vec;
use ark_ff::{AdditiveGroup, BigInteger, PrimeField};
use mina_curves::pasta::Fp;
use mina_poseidon::{
    constants::PlonkSpongeConstantsKimchi,
    pasta::fp_kimchi,
    permutation::poseidon_block_cipher,
    poseidon::ArithmeticSpongeParams,
};
use serde::{Deserialize, Serialize};

/// Standard Mina mainnet ledger depth (`2^35` leaves).
pub const MINA_LEDGER_DEPTH: u32 = 35;

/// Exact 20-byte Mina domain prefix for account leaf hashes.
pub const MINA_ACCOUNT_PREFIX: &str = "MinaAccount*********";

/// Exact 20-byte Mina domain prefix for `ProtocolState` `state_hash`.
pub const MINA_PROTO_STATE_PREFIX: &str = "MinaProtoState******";

/// Exact 20-byte Mina domain prefix for `ProtocolState.body` hash.
pub const MINA_PROTO_STATE_BODY_PREFIX: &str = "MinaProtoStateBody**";

/// Build the exact 20-byte Mina Merkle node prefix `"MinaMklTree{:03}******"` for `height`.
pub fn merkle_node_prefix_bytes(height: u32) -> [u8; 20] {
    assert!(height < 1000, "Mina Merkle height must be < 1000");
    let mut buf = *b"MinaMklTree000******";
    buf[11] = b'0' + ((height / 100) % 10) as u8;
    buf[12] = b'0' + ((height / 10) % 10) as u8;
    buf[13] = b'0' + (height % 10) as u8;
    buf
}

/// Convert a Mina ASCII domain prefix (`<= 31` bytes) into its canonical `Fp` element
/// (`from_le_bytes_mod_order` on the zero-right-padded 32-byte buffer).
pub fn prefix_bytes_to_fp(prefix: &[u8]) -> Fp {
    assert!(prefix.len() < 32, "Mina hash prefix must be < 32 bytes");
    let mut buf = [0u8; 32];
    buf[..prefix.len()].copy_from_slice(prefix);
    Fp::from_le_bytes_mod_order(&buf)
}

/// Canonical 32-byte little-endian encoding of a Pasta `Fp` field element.
pub fn fp_to_le_bytes(v: Fp) -> [u8; 32] {
    let mut out = [0u8; 32];
    let bytes = v.into_bigint().to_bytes_le();
    let n = bytes.len().min(32);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

/// Decode a 32-byte little-endian `Fp` field element.
pub fn fp_from_le_bytes(bytes: &[u8; 32]) -> Fp {
    Fp::from_le_bytes_mod_order(bytes)
}

/// Absorb `input` into a Kimchi Poseidon sponge state (rate 2, 55 full rounds).
pub fn poseidon_update(
    params: &ArithmeticSpongeParams<Fp, 55>,
    mut state: [Fp; 3],
    input: &[Fp],
) -> [Fp; 3] {
    if input.is_empty() {
        poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi, 55>(params, &mut state);
        return state;
    }
    for chunk in input.chunks(2) {
        state[0] += chunk[0];
        if chunk.len() == 2 {
            state[1] += chunk[1];
        }
        poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi, 55>(params, &mut state);
    }
    state
}

/// Precompute the initial sponge state after absorbing a Mina domain prefix.
pub fn poseidon_init_state(params: &ArithmeticSpongeParams<Fp, 55>, prefix: &[u8]) -> [Fp; 3] {
    poseidon_update(params, [Fp::ZERO; 3], &[prefix_bytes_to_fp(prefix)])
}

/// Mina's `hash_with_prefix(prefix, input)` over Kimchi `Fp` Poseidon.
pub fn hash_with_prefix(
    params: &ArithmeticSpongeParams<Fp, 55>,
    prefix: &[u8],
    input: &[Fp],
) -> Fp {
    let init = poseidon_init_state(params, prefix);
    poseidon_update(params, init, input)[0]
}

/// Mina's `ProtocolState.hash = hash_with_prefix("MinaProtoState******", [prev_state_hash, body_hash])`.
pub fn hash_protocol_state(prev_state_hash: Fp, body_hash: Fp) -> Fp {
    let params = fp_kimchi::static_params();
    hash_with_prefix(params, MINA_PROTO_STATE_PREFIX.as_bytes(), &[prev_state_hash, body_hash])
}

/// Precomputed Mina Poseidon ledger hasher (height-indexed `MinaMklTree{:03}******`
/// initial sponge states + `MinaAccount*********` initial state).
pub struct MinaLedgerHasher {
    params: &'static ArithmeticSpongeParams<Fp, 55>,
    account_init: [Fp; 3],
    node_inits: Vec<[Fp; 3]>,
}

impl MinaLedgerHasher {
    /// Precompute initial sponge states for `depth` Merkle heights (`0..depth`).
    pub fn new(depth: u32) -> Self {
        let params = fp_kimchi::static_params();
        let account_init = poseidon_init_state(params, MINA_ACCOUNT_PREFIX.as_bytes());
        let mut node_inits = Vec::with_capacity(depth as usize);
        for h in 0..depth {
            let prefix = merkle_node_prefix_bytes(h);
            node_inits.push(poseidon_init_state(params, &prefix));
        }
        Self { params, account_init, node_inits }
    }

    /// Hash one Mina account's packed field elements with `"MinaAccount*********"`.
    pub fn hash_account_fields(&self, packed_fields: &[Fp]) -> Fp {
        poseidon_update(self.params, self.account_init, packed_fields)[0]
    }

    /// Hash two children at Merkle height `height` (`0` = leaf pair) in **1 Poseidon permutation**.
    #[inline]
    pub fn hash_node(&self, height: u32, left: Fp, right: Fp) -> Fp {
        let mut state = self.node_inits[height as usize];
        state[0] += left;
        state[1] += right;
        poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi, 55>(self.params, &mut state);
        state[0]
    }

    /// Standard empty-account hash (`hash_account_fields(&[])`).
    pub fn empty_account_hash(&self) -> Fp {
        self.account_init[0]
    }

    /// Precompute empty-subtree hashes for levels `0..=depth`.
    pub fn empty_subtree_hashes(&self) -> Vec<Fp> {
        let depth = self.node_inits.len();
        let mut zeros = Vec::with_capacity(depth + 1);
        let mut cur = self.empty_account_hash();
        zeros.push(cur);
        for h in 0..depth {
            cur = self.hash_node(h as u32, cur, cur);
            zeros.push(cur);
        }
        zeros
    }
}

// ------------------------------------------------------------------ Keccak

#[cfg(not(feature = "keccak-precompile"))]
mod kc {
    use sha3::{Digest, Keccak256};
    pub struct H(Keccak256);
    impl H {
        #[inline]
        pub fn new() -> Self {
            H(Keccak256::new())
        }
        #[inline]
        pub fn update(&mut self, bytes: impl AsRef<[u8]>) {
            self.0.update(bytes.as_ref());
        }
        #[inline]
        pub fn finish(self) -> [u8; 32] {
            self.0.finalize().into()
        }
    }
}

#[cfg(feature = "keccak-precompile")]
mod kc {
    use tiny_keccak::{Hasher, Keccak};
    pub struct H(Keccak);
    impl H {
        #[inline]
        pub fn new() -> Self {
            H(Keccak::v256())
        }
        #[inline]
        pub fn update(&mut self, bytes: impl AsRef<[u8]>) {
            self.0.update(bytes.as_ref());
        }
        #[inline]
        pub fn finish(self) -> [u8; 32] {
            let mut out = [0u8; 32];
            self.0.finalize(&mut out);
            out
        }
    }
}

/// Keccak-256 of two 32-byte Merkle children (64 bytes = 1 Keccak permutation).
#[inline]
pub fn keccak_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = kc::H::new();
    h.update(left);
    h.update(right);
    h.finish()
}

/// Fixed 80-byte EVM-facing projection of a Mina account.
/// Combined with the 32-byte Poseidon leaf hash `H_p`, the total preimage is
/// `32 + 80 = 112` bytes (< 136-byte Keccak-256 rate = **1 Keccak permutation**).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvmAccountFields {
    /// Compressed Mina public key x-coordinate (32 bytes, little-endian).
    pub public_key_x: [u8; 32],
    /// Account balance in nanomina (u64).
    pub balance: u64,
    /// Account nonce (u32).
    pub nonce: u32,
    /// Bitflags: bit 0 = `public_key_is_odd`, bit 1 = `is_zkapp`.
    pub flags: u32,
    /// Hash of zkApp app_state[0..8] (or `[0u8; 32]` for non-zkApp accounts).
    pub zkapp_state_hash: [u8; 32],
}

impl EvmAccountFields {
    /// Fixed 80-byte ABI-packed encoding matching Solidity `abi.encodePacked`.
    pub fn to_packed_bytes(&self) -> [u8; 80] {
        let mut out = [0u8; 80];
        out[0..32].copy_from_slice(&self.public_key_x);
        out[32..40].copy_from_slice(&self.balance.to_be_bytes());
        out[40..44].copy_from_slice(&self.nonce.to_be_bytes());
        out[44..48].copy_from_slice(&self.flags.to_be_bytes());
        out[48..80].copy_from_slice(&self.zkapp_state_hash);
        out
    }
}

/// Optimization 3: `keccak256(H_p || evm_fields)` (112 bytes = 1 Keccak permutation).
pub fn keccak_bound_leaf(poseidon_leaf_le: &[u8; 32], evm_fields: &EvmAccountFields) -> [u8; 32] {
    let mut h = kc::H::new();
    h.update(poseidon_leaf_le);
    h.update(evm_fields.to_packed_bytes());
    h.finish()
}

/// Precompute empty Keccak subtree hashes for levels `0..=depth`.
pub fn empty_keccak_subtrees(depth: u32) -> Vec<[u8; 32]> {
    let mut zeros = Vec::with_capacity(depth as usize + 1);
    let mut cur = [0u8; 32];
    zeros.push(cur);
    for _ in 0..depth {
        cur = keccak_node(&cur, &cur);
        zeros.push(cur);
    }
    zeros
}

// -------------------------------------------------- Batched Multiproof + Dedup

/// One account update in a block or multi-block epoch settlement witness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUpdateWitness {
    /// Leaf index in Mina's depth-35 ledger.
    pub leaf_idx: u64,
    /// Monotonic sequence number within the block/epoch (used for dedup ordering).
    pub seq: u32,
    /// Packed `Fp` elements of the account BEFORE the update (32-byte LE each).
    pub old_account_fields: Vec<[u8; 32]>,
    /// Packed `Fp` elements of the account AFTER the update (32-byte LE each).
    pub new_account_fields: Vec<[u8; 32]>,
    /// EVM-facing fields BEFORE the update.
    pub old_evm_fields: EvmAccountFields,
    /// EVM-facing fields AFTER the update.
    pub new_evm_fields: EvmAccountFields,
}

/// Complete settlement witness passed alongside `VerifiableProof` to `o1-verifier-settlement`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettlementWitness {
    /// Ledger depth (35 on Mina mainnet).
    pub depth: u32,
    /// Mina block height being settled.
    pub block_height: u32,
    /// If `Some((prev_state_hash, body_hash))`, the guest asserts
    /// `hash_protocol_state(prev_state_hash, body_hash) == proof.app_state[0]`.
    pub protocol_state_preimage: Option<([u8; 32], [u8; 32])>,
    /// Expected Mina Poseidon ledger root BEFORE applying `updates`.
    pub old_poseidon_root: [u8; 32],
    /// Expected depth-`depth` Keccak global mirror root BEFORE applying `updates`.
    pub old_keccak_global_root: [u8; 32],
    /// Account updates in this block or epoch (may contain repeated hot accounts).
    pub updates: Vec<AccountUpdateWitness>,
    /// External Poseidon siblings consumed in sorted bottom-up multiproof order.
    pub poseidon_siblings: Vec<[u8; 32]>,
    /// External Keccak siblings consumed in sorted bottom-up multiproof order.
    pub keccak_siblings: Vec<[u8; 32]>,
}

/// Fixed 228-byte ABI-friendly public values committed by `o1-verifier-settlement`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementPublicValues {
    /// Verified Mina `ProtocolState` `state_hash` (`proof.app_state[0]`, 32-byte LE).
    pub state_hash: [u8; 32],
    /// Mina block height.
    pub block_height: u32,
    /// Mina Poseidon ledger root before updates.
    pub old_poseidon_root: [u8; 32],
    /// Mina Poseidon ledger root after updates.
    pub new_poseidon_root: [u8; 32],
    /// Depth-35 Keccak global mirror root before updates.
    pub old_keccak_global_root: [u8; 32],
    /// Depth-35 Keccak global mirror root after updates (Option 3).
    pub new_keccak_global_root: [u8; 32],
    /// Compact Keccak diff root over the `N` sorted updated leaves (Option 1).
    pub keccak_diff_root: [u8; 32],
}

impl SettlementPublicValues {
    pub const ABI_LEN: usize = 32 + 4 + 32 * 5; // 196 bytes

    /// Pack into a fixed 196-byte buffer for Solidity `abi.decode` / slicing.
    pub fn to_abi_bytes(&self) -> [u8; Self::ABI_LEN] {
        let mut out = [0u8; Self::ABI_LEN];
        out[0..32].copy_from_slice(&self.state_hash);
        out[32..36].copy_from_slice(&self.block_height.to_be_bytes());
        out[36..68].copy_from_slice(&self.old_poseidon_root);
        out[68..100].copy_from_slice(&self.new_poseidon_root);
        out[100..132].copy_from_slice(&self.old_keccak_global_root);
        out[132..164].copy_from_slice(&self.new_keccak_global_root);
        out[164..196].copy_from_slice(&self.keccak_diff_root);
        out
    }
}

/// Optimization 2: sort by `(leaf_idx, seq)` and collapse repeated touches to the
/// same `leaf_idx` into a single net transition (`first.old -> last.new`).
pub fn dedup_account_updates(updates: &[AccountUpdateWitness]) -> Vec<AccountUpdateWitness> {
    let mut sorted = updates.to_vec();
    sorted.sort_unstable_by_key(|u| (u.leaf_idx, u.seq));
    let mut out: Vec<AccountUpdateWitness> = Vec::with_capacity(sorted.len());
    for u in sorted {
        if let Some(last) = out.last_mut() {
            if last.leaf_idx == u.leaf_idx {
                last.new_account_fields = u.new_account_fields;
                last.new_evm_fields = u.new_evm_fields;
                last.seq = u.seq;
                continue;
            }
        }
        out.push(u);
    }
    out
}

/// Optimization 1: sorted bottom-up Merkle multiproof parameterized by `level`.
///
/// `hash_node(level, left, right)` receives the exact Merkle height `level` (`0..depth`)
/// so Mina's height-indexed `MinaMklTree{:03}******` prefix is applied with 1 permutation/node.
pub fn batch_merkle_update<H: Copy>(
    mut frontier: Vec<(u64, H, H)>,
    depth: u32,
    mut hash_node: impl FnMut(u32, &H, &H) -> H,
    mut sibling_fn: impl FnMut(u32, u64) -> H,
) -> (H, H) {
    assert!(!frontier.is_empty(), "frontier must be non-empty");
    let mut next_frontier: Vec<(u64, H, H)> = Vec::with_capacity(frontier.len());

    for level in 0..depth {
        next_frontier.clear();
        let mut i = 0;
        while i < frontier.len() {
            let (idx, old_v, new_v) = frontier[i];
            let parent = idx >> 1;
            if (idx & 1 == 0) && (i + 1 < frontier.len()) && (frontier[i + 1].0 == idx + 1) {
                let (_, old_r, new_r) = frontier[i + 1];
                let old_p = hash_node(level, &old_v, &old_r);
                let new_p = hash_node(level, &new_v, &new_r);
                next_frontier.push((parent, old_p, new_p));
                i += 2;
            } else {
                let sib = sibling_fn(level, idx ^ 1);
                let (old_p, new_p) = if idx & 1 == 0 {
                    (hash_node(level, &old_v, &sib), hash_node(level, &new_v, &sib))
                } else {
                    (hash_node(level, &sib, &old_v), hash_node(level, &sib, &new_v))
                };
                next_frontier.push((parent, old_p, new_p));
                i += 1;
            }
        }
        core::mem::swap(&mut frontier, &mut next_frontier);
    }

    debug_assert_eq!(frontier.len(), 1);
    (frontier[0].1, frontier[0].2)
}

/// Option 1: compact binary Keccak Merkle root over the `N` sorted updated leaves.
pub fn keccak_diff_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    let mut next: Vec<[u8; 32]> = Vec::with_capacity(level.len().div_ceil(2));
    let zero = [0u8; 32];
    while level.len() > 1 {
        next.clear();
        let mut i = 0;
        while i < level.len() {
            let left = &level[i];
            let right = if i + 1 < level.len() { &level[i + 1] } else { &zero };
            next.push(keccak_node(left, right));
            i += 2;
        }
        core::mem::swap(&mut level, &mut next);
    }
    level[0]
}

/// Verify and apply a complete Mina → Ethereum dual-root settlement witness,
/// anchored to `state_hash_from_proof` (`proof.app_state[0]`).
pub fn verify_and_apply_settlement(
    state_hash_from_proof: Fp,
    witness: &SettlementWitness,
) -> SettlementPublicValues {
    assert!((1..=40).contains(&witness.depth), "ledger depth out of range");

    // Anchor protocol state preimage to the verified Pickles proof statement if supplied.
    if let Some((prev_bytes, body_bytes)) = witness.protocol_state_preimage {
        let computed = hash_protocol_state(fp_from_le_bytes(&prev_bytes), fp_from_le_bytes(&body_bytes));
        assert_eq!(
            computed, state_hash_from_proof,
            "protocol state preimage does not match verified Pickles state_hash"
        );
    }

    let state_hash = fp_to_le_bytes(state_hash_from_proof);

    // Handle empty-diff block (e.g., checkpoint or no account changes).
    if witness.updates.is_empty() {
        return SettlementPublicValues {
            state_hash,
            block_height: witness.block_height,
            old_poseidon_root: witness.old_poseidon_root,
            new_poseidon_root: witness.old_poseidon_root,
            old_keccak_global_root: witness.old_keccak_global_root,
            new_keccak_global_root: witness.old_keccak_global_root,
            keccak_diff_root: [0u8; 32],
        };
    }

    // Opt 2: deduplicate hot accounts.
    let unique = dedup_account_updates(&witness.updates);

    // Phase 1 (Poseidon ledger — Opt 1 + Opt 5):
    let hasher = MinaLedgerHasher::new(witness.depth);
    let mut p_frontier: Vec<(u64, Fp, Fp)> = Vec::with_capacity(unique.len());
    let mut bound_pairs: Vec<(u64, [u8; 32], [u8; 32], EvmAccountFields, EvmAccountFields)> =
        Vec::with_capacity(unique.len());

    for u in &unique {
        let old_fps: Vec<Fp> = u.old_account_fields.iter().map(fp_from_le_bytes).collect();
        let new_fps: Vec<Fp> = u.new_account_fields.iter().map(fp_from_le_bytes).collect();
        let old_p = hasher.hash_account_fields(&old_fps);
        let new_p = hasher.hash_account_fields(&new_fps);
        p_frontier.push((u.leaf_idx, old_p, new_p));
        bound_pairs.push((
            u.leaf_idx,
            fp_to_le_bytes(old_p),
            fp_to_le_bytes(new_p),
            u.old_evm_fields,
            u.new_evm_fields,
        ));
    }

    let mut p_sib_cursor = 0usize;
    let (computed_old_p, new_p_root) = batch_merkle_update(
        p_frontier,
        witness.depth,
        |level, l, r| hasher.hash_node(level, *l, *r),
        |_level, _sib_idx| {
            let bytes = witness.poseidon_siblings[p_sib_cursor];
            p_sib_cursor += 1;
            fp_from_le_bytes(&bytes)
        },
    );
    assert_eq!(
        p_sib_cursor,
        witness.poseidon_siblings.len(),
        "unused poseidon siblings in settlement witness"
    );
    assert_eq!(
        fp_to_le_bytes(computed_old_p),
        witness.old_poseidon_root,
        "computed old Poseidon root does not match witness.old_poseidon_root"
    );

    // Phase 2 (Keccak dual-root mirror — Opt 1 + Opt 3 + Opt 5):
    let mut k_frontier: Vec<(u64, [u8; 32], [u8; 32])> = Vec::with_capacity(bound_pairs.len());
    let mut diff_leaves: Vec<[u8; 32]> = Vec::with_capacity(bound_pairs.len());
    for (leaf_idx, old_p_bytes, new_p_bytes, old_evm, new_evm) in &bound_pairs {
        let old_k = keccak_bound_leaf(old_p_bytes, old_evm);
        let new_k = keccak_bound_leaf(new_p_bytes, new_evm);
        k_frontier.push((*leaf_idx, old_k, new_k));
        diff_leaves.push(new_k);
    }

    let diff_root = keccak_diff_root(&diff_leaves);

    let mut k_sib_cursor = 0usize;
    let (computed_old_k, new_k_root) = batch_merkle_update(
        k_frontier,
        witness.depth,
        |_level, l, r| keccak_node(l, r),
        |_level, _sib_idx| {
            let sib = witness.keccak_siblings[k_sib_cursor];
            k_sib_cursor += 1;
            sib
        },
    );
    assert_eq!(
        k_sib_cursor,
        witness.keccak_siblings.len(),
        "unused keccak siblings in settlement witness"
    );
    assert_eq!(
        computed_old_k, witness.old_keccak_global_root,
        "computed old Keccak global root does not match witness.old_keccak_global_root"
    );

    SettlementPublicValues {
        state_hash,
        block_height: witness.block_height,
        old_poseidon_root: witness.old_poseidon_root,
        new_poseidon_root: fp_to_le_bytes(new_p_root),
        old_keccak_global_root: witness.old_keccak_global_root,
        new_keccak_global_root: new_k_root,
        keccak_diff_root: diff_root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn build_dense_poseidon(hasher: &MinaLedgerHasher, leaves: &[Fp], depth: u32) -> Vec<Vec<Fp>> {
        let mut levels = vec![leaves.to_vec()];
        for h in 0..depth {
            let prev = levels.last().unwrap();
            let mut next = Vec::with_capacity(prev.len() / 2);
            for pair in prev.chunks_exact(2) {
                next.push(hasher.hash_node(h, pair[0], pair[1]));
            }
            levels.push(next);
        }
        levels
    }

    fn build_dense_keccak(leaves: &[[u8; 32]], depth: u32) -> Vec<Vec<[u8; 32]>> {
        let mut levels = vec![leaves.to_vec()];
        for _ in 0..depth {
            let prev = levels.last().unwrap();
            let mut next = Vec::with_capacity(prev.len() / 2);
            for pair in prev.chunks_exact(2) {
                next.push(keccak_node(&pair[0], &pair[1]));
            }
            levels.push(next);
        }
        levels
    }

    #[test]
    fn mina_domain_prefixes_are_exact_20_bytes() {
        assert_eq!(MINA_ACCOUNT_PREFIX.len(), 20);
        assert_eq!(MINA_PROTO_STATE_PREFIX.len(), 20);
        assert_eq!(MINA_PROTO_STATE_BODY_PREFIX.len(), 20);
        assert_eq!(&merkle_node_prefix_bytes(0), b"MinaMklTree000******");
        assert_eq!(&merkle_node_prefix_bytes(34), b"MinaMklTree034******");
    }

    #[test]
    fn end_to_end_settlement_matches_dense_mina_and_keccak_trees() {
        let depth = 6u32;
        let n_leaves = 1usize << depth;
        let hasher = MinaLedgerHasher::new(depth);

        let make_account = |seed: u64| -> (Vec<[u8; 32]>, EvmAccountFields) {
            let fields = vec![
                fp_to_le_bytes(Fp::from(seed + 1)),
                fp_to_le_bytes(Fp::from(seed * 7 + 3)),
                fp_to_le_bytes(Fp::from(seed * 13 + 5)),
                fp_to_le_bytes(Fp::from(seed * 29 + 11)),
            ];
            let evm = EvmAccountFields {
                public_key_x: fp_to_le_bytes(Fp::from(seed + 1)),
                balance: 1_000_000 + seed * 100,
                nonce: seed as u32,
                flags: (seed & 1) as u32,
                zkapp_state_hash: fp_to_le_bytes(Fp::from(seed * 29 + 11)),
            };
            (fields, evm)
        };

        let mut old_p_leaves = Vec::with_capacity(n_leaves);
        let mut old_k_leaves = Vec::with_capacity(n_leaves);
        for i in 0..n_leaves as u64 {
            let (fields, evm) = make_account(i);
            let fps: Vec<Fp> = fields.iter().map(fp_from_le_bytes).collect();
            let hp = hasher.hash_account_fields(&fps);
            old_p_leaves.push(hp);
            old_k_leaves.push(keccak_bound_leaf(&fp_to_le_bytes(hp), &evm));
        }

        let old_p_levels = build_dense_poseidon(&hasher, &old_p_leaves, depth);
        let old_k_levels = build_dense_keccak(&old_k_leaves, depth);

        // 6 raw updates touching 4 unique accounts (leaf 5 and leaf 42 touched twice).
        let raw_specs = [
            (5u64, 0u32, 5u64, 1005u64),
            (4u64, 1u32, 4u64, 1004u64),
            (5u64, 2u32, 1005u64, 2005u64),
            (42u64, 3u32, 42u64, 1042u64),
            (43u64, 4u32, 43u64, 1043u64),
            (42u64, 5u32, 1042u64, 3042u64),
        ];
        let mut updates = Vec::new();
        let mut new_p_leaves = old_p_leaves.clone();
        let mut new_k_leaves = old_k_leaves.clone();
        for (idx, seq, old_s, new_s) in raw_specs {
            let (old_f, old_e) = make_account(old_s);
            let (new_f, new_e) = make_account(new_s);
            let new_fps: Vec<Fp> = new_f.iter().map(fp_from_le_bytes).collect();
            let new_hp = hasher.hash_account_fields(&new_fps);
            new_p_leaves[idx as usize] = new_hp;
            new_k_leaves[idx as usize] = keccak_bound_leaf(&fp_to_le_bytes(new_hp), &new_e);
            updates.push(AccountUpdateWitness {
                leaf_idx: idx,
                seq,
                old_account_fields: old_f,
                new_account_fields: new_f,
                old_evm_fields: old_e,
                new_evm_fields: new_e,
            });
        }

        let expected_new_p = build_dense_poseidon(&hasher, &new_p_leaves, depth)[depth as usize][0];
        let expected_new_k = build_dense_keccak(&new_k_leaves, depth)[depth as usize][0];

        // Collect external siblings in sorted bottom-up order.
        let unique = dedup_account_updates(&updates);
        assert_eq!(unique.len(), 4);
        let mut p_sibs = Vec::new();
        let mut k_sibs = Vec::new();
        let mut frontier: Vec<u64> = unique.iter().map(|u| u.leaf_idx).collect();
        for level in 0..depth {
            let mut next = Vec::new();
            let mut i = 0;
            while i < frontier.len() {
                let idx = frontier[i];
                if (idx & 1 == 0) && (i + 1 < frontier.len()) && (frontier[i + 1] == idx + 1) {
                    next.push(idx >> 1);
                    i += 2;
                } else {
                    let sib_idx = (idx ^ 1) as usize;
                    p_sibs.push(fp_to_le_bytes(old_p_levels[level as usize][sib_idx]));
                    k_sibs.push(old_k_levels[level as usize][sib_idx]);
                    next.push(idx >> 1);
                    i += 1;
                }
            }
            frontier = next;
        }

        let prev_state = Fp::from(12345u64);
        let body_hash = Fp::from(67890u64);
        let state_hash = hash_protocol_state(prev_state, body_hash);

        let witness = SettlementWitness {
            depth,
            block_height: 412_000,
            protocol_state_preimage: Some((fp_to_le_bytes(prev_state), fp_to_le_bytes(body_hash))),
            old_poseidon_root: fp_to_le_bytes(old_p_levels[depth as usize][0]),
            old_keccak_global_root: old_k_levels[depth as usize][0],
            updates,
            poseidon_siblings: p_sibs,
            keccak_siblings: k_sibs,
        };

        let pv = verify_and_apply_settlement(state_hash, &witness);
        assert_eq!(pv.state_hash, fp_to_le_bytes(state_hash));
        assert_eq!(pv.block_height, 412_000);
        assert_eq!(pv.new_poseidon_root, fp_to_le_bytes(expected_new_p));
        assert_eq!(pv.new_keccak_global_root, expected_new_k);

        // Verify keccak_diff_root matches the 4 unique sorted new leaves.
        let expected_diff_leaves: Vec<[u8; 32]> =
            unique.iter().map(|u| new_k_leaves[u.leaf_idx as usize]).collect();
        assert_eq!(pv.keccak_diff_root, keccak_diff_root(&expected_diff_leaves));
        assert_eq!(pv.to_abi_bytes().len(), SettlementPublicValues::ABI_LEN);
    }
}
