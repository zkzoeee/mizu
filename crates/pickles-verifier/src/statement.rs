//! In-guest binding between the Pickles proof statement, deferred IPA
//! accumulators, branch metadata, and the two Pickles message digests verified
//! by the wrap proof.
//!
//! The host-side OCaml wire conversion reconstructs both
//! `messages_for_next_step_proof_digest` and `messages_for_next_wrap_proof_digest`
//! because the serialized proof skeleton erases them. Recomputing and binding
//! both digests and all deferred-accumulator invariants in the guest prevents a
//! malicious prover/host from decoupling `app_state`, `challenge_polynomial_commitment`,
//! `prev_step_sgs`, `wrap_proof.prev_challenges`, or branch metadata from the
//! verified wrap proof.

use alloc::format;
use alloc::vec::Vec;

use blake2::{Blake2s256, Digest};
use mina_curves::pasta::Pallas;
use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::pasta::{fp_kimchi, fq_kimchi, FULL_ROUNDS};
use mina_poseidon::poseidon::{ArithmeticSponge, Sponge};
use mina_poseidon::sponge::ScalarChallenge;
use poly_commitment::ipa::endos;

use crate::types::{
    StepField, VerifiableProof, Verifier, WrapField, STEP_IPA_ROUNDS, WRAP_IPA_ROUNDS,
};

/// `Wrap_hack.Padded_length.n` — the wrap proof always carries this many
/// previous-challenge slots, front-padded with dummy wrap challenges.
pub const PADDED_LENGTH: usize = 2;

/// Maximum valid radix-2 domain log2 for Pasta fields (two-adicity = 32).
pub const MAX_STEP_DOMAIN_LOG2: usize = 32;

/// Decode the Pickles `proofs_verified_mask` prefix encoding:
/// N0 = `[false, false]`, N1 = `[false, true]`, N2 = `[true, true]`.
/// Rejects the invalid `[true, false]` bit pattern.
pub fn proofs_verified_count(mask: [bool; 2]) -> Option<usize> {
    match mask {
        [false, false] => Some(0),
        [false, true] => Some(1),
        [true, true] => Some(2),
        [true, false] => None,
    }
}

/// The 15 dummy wrap IPA challenges, endo-expanded — port of OCaml
/// `Dummy.Ipa.Wrap.challenges` (`ro.ml`). Each raw challenge `chal_i` is the
/// low 128 bits (LE) of `blake2s256("chal_i")`; the `Ro` monad draws them in
/// counter order `1..=15` and stores them reversed.
pub(crate) fn dummy_ipa_wrap_expanded(wrap_endo: &WrapField) -> [WrapField; WRAP_IPA_ROUNDS] {
    let mut v: Vec<WrapField> = (1..=WRAP_IPA_ROUNDS)
        .map(|i| {
            let digest = Blake2s256::digest(format!("chal_{i}").as_bytes());
            let lo = u64::from_le_bytes(digest[0..8].try_into().unwrap());
            let hi = u64::from_le_bytes(digest[8..16].try_into().unwrap());
            ScalarChallenge::<WrapField>::from_limbs([lo, hi]).to_field(wrap_endo)
        })
        .collect();
    v.reverse();
    v.try_into()
        .unwrap_or_else(|_| unreachable!("exactly WRAP_IPA_ROUNDS produced"))
}

fn wrap_vk_step_fields(verifier: &Verifier) -> Option<Vec<StepField>> {
    let vk = &verifier.wrap_vk;
    let index_comms = [
        &vk.generic_comm,
        &vk.psm_comm,
        &vk.complete_add_comm,
        &vk.mul_comm,
        &vk.emul_comm,
        &vk.endomul_scalar_comm,
    ];

    vk.sigma_comm
        .iter()
        .chain(vk.coefficients_comm.iter())
        .chain(index_comms)
        .try_fold(Vec::with_capacity((7 + 15 + 6) * 2), |mut out, pc| {
            let point = pc.chunks.first()?;
            out.push(point.x);
            out.push(point.y);
            Some(out)
        })
}

/// Recompute `Common.hash_messages_for_next_step_proof` from the application
/// statement and previous-proof commitments carried in [`VerifiableProof`].
pub fn app_state_digest(verifier: &Verifier, proof: &VerifiableProof) -> Option<StepField> {
    if proof.prev_step_sgs.len() != proof.old_bulletproof_challenges.len() {
        return None;
    }

    let vk_fields = wrap_vk_step_fields(verifier)?;
    let mut inputs = Vec::with_capacity(
        vk_fields.len() + proof.app_state.len() + proof.prev_step_sgs.len() * (2 + STEP_IPA_ROUNDS),
    );
    inputs.extend_from_slice(&vk_fields);
    inputs.extend_from_slice(&proof.app_state);
    for (sg, challenges) in proof
        .prev_step_sgs
        .iter()
        .zip(proof.old_bulletproof_challenges.iter())
    {
        if sg.infinity {
            return None;
        }
        inputs.push(sg.x);
        inputs.push(sg.y);
        inputs.extend_from_slice(challenges);
    }

    let mut sponge: ArithmeticSponge<StepField, PlonkSpongeConstantsKimchi, FULL_ROUNDS> =
        Sponge::new(fp_kimchi::static_params());
    sponge.absorb(&inputs);
    Some(sponge.squeeze())
}

/// Recompute `Wrap_hack.hash_messages_for_next_wrap_proof` from the wrap
/// proof's padded `prev_challenges` and the step IPA accumulator commitment
/// `challenge_polynomial_commitment`.
pub fn wrap_messages_digest(proof: &VerifiableProof) -> Option<WrapField> {
    if proof.challenge_polynomial_commitment.infinity
        || proof.wrap_proof.prev_challenges.len() != PADDED_LENGTH
    {
        return None;
    }
    let mut inputs = Vec::with_capacity(PADDED_LENGTH * WRAP_IPA_ROUNDS + 2);
    for pc in &proof.wrap_proof.prev_challenges {
        if pc.chals.len() != WRAP_IPA_ROUNDS {
            return None;
        }
        inputs.extend_from_slice(&pc.chals);
    }
    inputs.push(proof.challenge_polynomial_commitment.x);
    inputs.push(proof.challenge_polynomial_commitment.y);

    let mut sponge: ArithmeticSponge<WrapField, PlonkSpongeConstantsKimchi, FULL_ROUNDS> =
        Sponge::new(fq_kimchi::static_params());
    sponge.absorb(&inputs);
    Some(sponge.squeeze())
}

pub fn check_app_state_binding(verifier: &Verifier, proof: &VerifiableProof) -> bool {
    app_state_digest(verifier, proof) == Some(proof.messages_for_next_step_proof_digest)
}

/// Full in-guest binding check for a Pickles proof:
/// 1. Outer wrap opening round count equals `WRAP_IPA_ROUNDS`.
/// 2. `branch_data.proofs_verified_mask` is valid (`N0`/`N1`/`N2`), and
///    `step_domain_log2` is in range and equals `branch_data.domain_log2`.
/// 3. `prev_step_sgs` and `old_bulletproof_challenges` match `mpv`, and all
///    accumulator commitments are finite points.
/// 4. `wrap_proof.prev_challenges` has length `PADDED_LENGTH` (2), each with
///    `WRAP_IPA_ROUNDS` challenges and a single finite commitment chunk; the
///    leading `PADDED_LENGTH - mpv` slots carry `dummy_ipa_wrap_expanded`, and
///    the trailing `mpv` slots carry `prev_step_sgs`.
/// 5. Both `messages_for_next_step_proof_digest` and
///    `messages_for_next_wrap_proof_digest` match their in-guest Poseidon
///    recomputations.
pub fn check_proof_binding(verifier: &Verifier, proof: &VerifiableProof) -> bool {
    if proof.wrap_proof.proof.lr.len() != WRAP_IPA_ROUNDS {
        return false;
    }

    let Some(mpv) = proofs_verified_count(proof.branch_data.proofs_verified_mask) else {
        return false;
    };

    if proof.step_domain_log2 > MAX_STEP_DOMAIN_LOG2
        || proof.step_domain_log2 >= usize::BITS as usize
        || StepField::from(proof.step_domain_log2 as u64) != proof.branch_data.domain_log2
    {
        return false;
    }

    if proof.prev_step_sgs.len() != mpv
        || proof.old_bulletproof_challenges.len() != mpv
        || proof.challenge_polynomial_commitment.infinity
        || proof.wrap_proof.prev_challenges.len() != PADDED_LENGTH
    {
        return false;
    }

    for pc in &proof.wrap_proof.prev_challenges {
        if pc.chals.len() != WRAP_IPA_ROUNDS
            || pc.comm.chunks.len() != 1
            || pc.comm.chunks[0].infinity
        {
            return false;
        }
    }

    let pad = PADDED_LENGTH - mpv;
    if pad > 0 {
        let wrap_endo = endos::<Pallas>().1;
        let dummy = dummy_ipa_wrap_expanded(&wrap_endo);
        for pc in &proof.wrap_proof.prev_challenges[..pad] {
            if pc.chals.as_slice() != dummy.as_slice() {
                return false;
            }
        }
    }

    for (pc, sg) in proof.wrap_proof.prev_challenges[pad..]
        .iter()
        .zip(proof.prev_step_sgs.iter())
    {
        if sg.infinity || pc.comm.chunks[0] != *sg {
            return false;
        }
    }

    check_app_state_binding(verifier, proof)
        && wrap_messages_digest(proof) == Some(proof.messages_for_next_wrap_proof_digest)
}
