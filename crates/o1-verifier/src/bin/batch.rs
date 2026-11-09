//! Optional bounded batch guest. Same embedded key; separate program identity.
#![no_main]
sp1_zkvm::entrypoint!(main);
#[path = "../atomic_shims.rs"]
mod atomic_shims;
use pickles_verifier::{serialize::decode_embedded_verifier, types::VerifiableProof};
#[repr(C, align(8))]
struct Aligned<T: ?Sized>(T);
static VERIFIER_BYTES: &Aligned<[u8]> = &Aligned(*include_bytes!(concat!(env!("OUT_DIR"), "/verifier.bin")));
static DIGEST: &[u8; 32] = include_bytes!(concat!(env!("OUT_DIR"), "/vk_transcript_digest.bin"));
fn main() {
    // Validate the count before allocating. Proofs use separate stdin items.
    let count: u32 = sp1_zkvm::io::read();
    assert!((1..=8).contains(&count), "batch size must be 1..=8");
    sp1_zkvm::io::write(1, b"cycle-tracker-report-start:setup\n");
    let verifier = decode_embedded_verifier(&VERIFIER_BYTES.0, DIGEST);
    let proofs: Vec<VerifiableProof> = (0..count).map(|_| sp1_zkvm::io::read()).collect();
    sp1_zkvm::io::write(1, b"cycle-tracker-report-end:setup\n");
    assert!(verifier.verify_batch(&proofs), "batch verification failed");
    sp1_zkvm::io::commit(&true);
}
