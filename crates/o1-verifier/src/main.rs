//! Verify one Pickles proof and commit a boolean, following the upstream guest ABI.

#![no_main]
sp1_zkvm::entrypoint!(main);

use pickles_verifier::serialize::{
    decode_embedded_verifier, decode_embedded_verifier_for_proof, decode_verifier_blob,
};
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::verify;

#[cfg(target_os = "zkvm")]
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
    let eager_proof: Option<VerifiableProof> = if matches!(option_env!("ZEKO_WRAP_SRS_PREFIX"), Some("1"))
        && matches!(option_env!("ZEKO_PRECOMPUTED_VK_DIGEST"), Some("1"))
    {
        Some(sp1_zkvm::io::read())
    } else {
        None
    };
    let verifier = if matches!(option_env!("ZEKO_PRECOMPUTED_VK_DIGEST"), Some("1")) {
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
    tracker(b"cycle-tracker-report-end:setup\n");

    tracker(b"cycle-tracker-report-start:verify\n");
    let proof_valid = if let Some(fixed) = &verifier {
        fixed.verify(&proof)
    } else {
        verify(runtime_verifier.as_ref().unwrap(), &proof)
    };
    tracker(b"cycle-tracker-report-end:verify\n");

    assert!(proof_valid, "Pickles proof verification failed");
    #[cfg(not(feature = "statement-digest"))]
    sp1_zkvm::io::commit(&proof_valid);
    #[cfg(feature = "statement-digest")]
    {
        let vk_id = include_bytes!(concat!(env!("OUT_DIR"), "/vk_hash.bin"));
        let digest = pickles_verifier::attestation::statement_digest(vk_id, &proof.app_state);
        sp1_zkvm::io::commit_slice(&digest);
    }
}
