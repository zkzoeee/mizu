//! Diverse producer proofs, same VK, through the complete embedded batch path.
use ark_ff::Field;
use mina_curves::pasta::{Pallas, Vesta};
use pickles_verifier::{
    accumulator_check, accumulator_check_batch,
    serialize::{decode_embedded_verifier, embedded_wrap_digest, encode_embedded_verifier_blob},
    statement::app_state_digest,
    types::{StepField, VerifiableProof, Verifier, WrapField, WrapVerifierIndex},
    wire::{parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof},
};
use poly_commitment::precomputed_srs::get_srs;
use std::{path::PathBuf, sync::Arc};

fn load(name: &str) -> (WrapVerifierIndex, VerifiableProof) {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    let read = |file| std::fs::read_to_string(directory.join(file)).unwrap();
    let key = parse_wrap_vk(&read("vk.serde.json")).unwrap();
    let proof = parse_wrap_proof(&read("proof.serde.json")).unwrap();
    let statement = parse_app_statement_fields(&read("app_statement.json")).unwrap();
    let input = OcamlProof::parse(&read("public_input_skeleton.json"))
        .unwrap()
        .into_verifiable(proof, &key, &statement)
        .unwrap();
    (key, input)
}

#[test]
fn distinct_proofs_preserve_full_embedded_batch_verification_and_rejection() {
    let (key, p0) = load("simplechain/wrap0");
    let (key1, p1) = load("simplechain/wrap1");
    let (key2, p2) = load("simplechain/wrap2");
    let digest = embedded_wrap_digest(&key);
    assert_eq!(digest, embedded_wrap_digest(&key1));
    assert_eq!(digest, embedded_wrap_digest(&key2));
    assert_ne!(p0.raw_bulletproof_challenges, p1.raw_bulletproof_challenges);
    assert_ne!(
        p0.challenge_polynomial_commitment,
        p1.challenge_polynomial_commitment
    );
    let verifier = Verifier::new(
        key,
        Arc::new(get_srs::<Pallas>()),
        Arc::new(get_srs::<Vesta>()),
        1,
    );
    let proofs = vec![p0, p1, p2];
    assert!(proofs.iter().all(|p| accumulator_check(&verifier, p)));
    assert!(accumulator_check_batch(&verifier, &proofs));
    let bytes = encode_embedded_verifier_blob(
        &verifier.vesta_srs,
        &verifier.wrap_vk.srs,
        1,
        &verifier.wrap_vk,
    );
    let embedded = decode_embedded_verifier(
        Box::leak(bytes.into_boxed_slice()),
        Box::leak(Box::new(digest)),
    );
    assert!(!embedded.verify_batch(&[]));
    assert!(embedded.verify_batch(&proofs));
    for index in 0..proofs.len() {
        let mut changed = proofs.clone();
        changed[index].raw_bulletproof_challenges[0] += StepField::ONE;
        assert!(!embedded.verify_batch(&changed), "challenge at {index}");
        let mut changed = proofs.clone();
        changed[index].wrap_proof.proof.z1 += WrapField::ONE;
        assert!(!embedded.verify_batch(&changed), "opening at {index}");
    }
    let mut swapped = proofs.clone();
    let commitment = swapped[0].challenge_polynomial_commitment;
    swapped[0].challenge_polynomial_commitment = swapped[1].challenge_polynomial_commitment;
    swapped[1].challenge_polynomial_commitment = commitment;
    assert!(!accumulator_check_batch(&verifier, &swapped));
    assert!(!embedded.verify_batch(&swapped));
    let mut rehashed = proofs.clone();
    rehashed[1].app_state[0] += StepField::ONE;
    rehashed[1].messages_for_next_step_proof_digest =
        app_state_digest(&verifier, &rehashed[1]).unwrap();
    assert!(
        !embedded.verify_batch(&rehashed),
        "rehashed statement must fail the wrap check"
    );
    let mut wrong_key = proofs.clone();
    wrong_key[2] = load("nrr").1;
    assert!(
        !embedded.verify_batch(&wrong_key),
        "another VK must not join this batch"
    );
}
