//! Combined Mina → Ethereum settlement guest:
//! 1. Verifies one Mina mainnet Pickles/kimchi blockchain SNARK (`mizu/dev` compact `w=16`).
//! 2. Verifies & applies the batched dual-root ledger diff (`pickles_verifier::mirror`),
//!    anchored to `proof.app_state[0]` (`ProtocolState` `state_hash`).
//! 3. Commits fixed 196-byte `SettlementPublicValues`:
//!    `(stateHash, blockHeight, oldPoseidonRoot, newPoseidonRoot,
//!      oldKeccakGlobalRoot, newKeccakGlobalRoot, keccakDiffRoot)`.

#![no_main]
sp1_zkvm::entrypoint!(main);

use pickles_verifier::mirror::{verify_and_apply_settlement, SettlementWitness};
use pickles_verifier::serialize::{
    decode_embedded_verifier, decode_embedded_verifier_for_proof, decode_verifier_blob,
};
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::verify;

#[cfg(target_os = "zkvm")]
#[path = "../atomic_shims.rs"]
mod atomic_shims;

#[repr(C, align(8))]
struct Aligned<T: ?Sized>(T);

static VERIFIER_BYTES: &Aligned<[u8]> =
    &Aligned(*include_bytes!(concat!(env!("OUT_DIR"), "/verifier.bin")));
static VK_TRANSCRIPT_DIGEST: &[u8; 32] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vk_transcript_digest.bin"));

fn tracker(line: &[u8]) {
    sp1_zkvm::io::write(1, line);
}

pub fn main() {
    tracker(b"cycle-tracker-report-start:setup\n");
    let eager_proof: Option<VerifiableProof> =
        if matches!(option_env!("MIZU_WRAP_SRS_PREFIX"), Some("1"))
            && matches!(option_env!("MIZU_PRECOMPUTED_VK_DIGEST"), Some("1"))
        {
            Some(sp1_zkvm::io::read())
        } else {
            None
        };
    let verifier = if matches!(option_env!("MIZU_PRECOMPUTED_VK_DIGEST"), Some("1")) {
        Some(if let Some(proof) = eager_proof.as_ref() {
            decode_embedded_verifier_for_proof(&VERIFIER_BYTES.0, VK_TRANSCRIPT_DIGEST, proof)
        } else {
            decode_embedded_verifier(&VERIFIER_BYTES.0, VK_TRANSCRIPT_DIGEST)
        })
    } else {
        None
    };
    let runtime_verifier = if verifier.is_none() {
        Some(decode_verifier_blob(&VERIFIER_BYTES.0))
    } else {
        None
    };
    let proof = eager_proof.unwrap_or_else(|| sp1_zkvm::io::read());
    let settlement: SettlementWitness = sp1_zkvm::io::read();
    tracker(b"cycle-tracker-report-end:setup\n");

    tracker(b"cycle-tracker-report-start:verify\n");
    let proof_valid = if let Some(fixed) = &verifier {
        fixed.verify(&proof)
    } else {
        verify(runtime_verifier.as_ref().unwrap(), &proof)
    };
    tracker(b"cycle-tracker-report-end:verify\n");
    assert!(proof_valid, "Pickles proof verification failed");

    tracker(b"cycle-tracker-report-start:mirror-settlement\n");
    assert!(!proof.app_state.is_empty(), "Pickles proof has empty app_state");
    let pv = verify_and_apply_settlement(proof.app_state[0], &settlement);
    tracker(b"cycle-tracker-report-end:mirror-settlement\n");

    sp1_zkvm::io::commit_slice(&pv.to_abi_bytes());
}
