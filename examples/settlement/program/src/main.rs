//! Optional settlement receipt guest layered on Pickles verification.
//!
//! This intentionally follows the o1 `o1js-to-zkvm` verifier shape: the Pickles
//! verifier blob is baked at build time from the wrap VK, and the guest only
//! reads a `VerifiableProof`, verifies it, and commits settlement public values.

#![no_main]
sp1_zkvm::entrypoint!(main);

use pickles_verifier::serialize::{
    decode_embedded_verifier, decode_embedded_verifier_for_proof, decode_verifier_blob,
};
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::verify;
use settlement_program::derive_receipt_bytes;
use mizu_settlement_types::SettlementWitnessV1;

#[repr(C, align(8))]
struct Aligned<T: ?Sized>(T);

static VERIFIER_BYTES: &Aligned<[u8]> =
    &Aligned(*include_bytes!(concat!(env!("OUT_DIR"), "/verifier.bin")));
static VK_TRANSCRIPT_DIGEST: &[u8; 32] =
    include_bytes!(concat!(env!("OUT_DIR"), "/vk_transcript_digest.bin"));
static VK_HASH: &[u8; 32] = include_bytes!(concat!(env!("OUT_DIR"), "/vk_hash.bin"));

fn tracker(line: &[u8]) {
    sp1_zkvm::io::write(1, line);
}

fn commit_zkapp_public_values(proof_valid: bool) {
    let empty_state = [[0u8; 32]; 8];
    let empty_action_state = [0u8; 32];

    let mut encoded = Vec::with_capacity(1 + 32 + 8 * 32 + 8 * 32 + 32);
    encoded.push(u8::from(proof_valid));
    encoded.extend_from_slice(VK_HASH);

    for field in &empty_state {
        encoded.extend_from_slice(field);
    }

    for field in &empty_state {
        encoded.extend_from_slice(field);
    }

    encoded.extend_from_slice(&empty_action_state);
    debug_assert_eq!(encoded.len(), 577);

    sp1_zkvm::io::commit_slice(&encoded);
}

pub fn main() {
    tracker(b"cycle-tracker-report-start:setup\n");
    let eager_proof: Option<VerifiableProof> = if matches!(option_env!("MIZU_WRAP_SRS_PREFIX"), Some("1"))
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
    let settlement: Option<SettlementWitnessV1> = sp1_zkvm::io::read();
    tracker(b"cycle-tracker-report-end:setup\n");

    tracker(b"cycle-tracker-report-start:verify\n");
    let proof_valid = if let Some(fixed) = &verifier {
        fixed.verify(&proof)
    } else {
        verify(runtime_verifier.as_ref().unwrap(), &proof)
    };
    tracker(b"cycle-tracker-report-end:verify\n");

    assert!(proof_valid, "Pickles proof verification failed");
    match settlement {
        Some(witness) => {
            let public_values = derive_receipt_bytes(&proof, witness, *VK_HASH);
            sp1_zkvm::io::commit_slice(&public_values);
        }
        None => commit_zkapp_public_values(proof_valid),
    }
}
