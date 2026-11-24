//! Experimental Fiat–Shamir combination of accumulator equations.
//! Fixed setup/table ownership is unchanged. Never truncate a coefficient vector.
use alloc::vec::Vec;
use ark_ec::{AffineRepr, CurveGroup, VariableBaseMSM};
use ark_ff::{BigInteger, One, PrimeField, Zero};
use ark_serialize::CanonicalSerialize;
use blake2::{Blake2s256, Digest};
use mina_curves::pasta::{Vesta, canonical_words::{CanonicalPasta, CanonicalPastaRhs}};
use mina_poseidon::sponge::ScalarChallenge;
use crate::{types::{StepField, VerifiableProof, Verifier}, fixed_srs::FixedVestaSrs};

pub(crate) fn challenge(verifier: &Verifier, proofs: &[VerifiableProof], digest: Option<StepField>) -> StepField {
    let mut hash = Blake2s256::new();
    hash.update(b"mizu/batch-accumulator/v1");
    hash.update((proofs.len() as u64).to_le_bytes());
    hash.update((verifier.step_srs_length_log2 as u64).to_le_bytes());
    let digest = digest.map(|d| d.into_bigint().to_bytes_le())
        .unwrap_or_else(|| crate::serialize::embedded_wrap_digest(&verifier.wrap_vk).to_vec());
    hash.update(digest);
    for proof in proofs {
        let mut bytes = Vec::new();
        proof.raw_bulletproof_challenges.serialize_compressed(&mut bytes).expect("challenges");
        proof.challenge_polynomial_commitment.serialize_compressed(&mut bytes).expect("commitment");
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    // Deterministic rejection sampling excludes the zero challenge. The full
    // ordered batch is committed before the counter; no fresh external RNG.
    let seed = hash.finalize();
    for counter in 0u64.. {
        let mut hash = Blake2s256::new();
        hash.update(seed); hash.update(counter.to_le_bytes());
        let r = StepField::from_le_bytes_mod_order(&hash.finalize());
        if !r.is_zero() { return r; }
    }
    unreachable!()
}

pub(crate) fn check(verifier: &Verifier, proofs: &[VerifiableProof], g: &[Vesta],
    table: Option<&FixedVestaSrs<'_>>, digest: Option<StepField>) -> bool {
    if proofs.is_empty() { return false; }
    if proofs.len() == 1 {
        return match table {
            Some(table) => crate::accumulator_check_fixed(verifier, &proofs[0], table),
            None => crate::accumulator_check_with_generators(verifier, &proofs[0], g),
        };
    }
    crate::track_cycles(b"cycle-tracker-report-start:pickles-accumulator\n");
    let result = (|| {
        if verifier.step_srs_length_log2 >= usize::BITS as usize { return false; }
        let expected = 1usize << verifier.step_srs_length_log2;
        if let Some(t) = table {
            if t.len() != expected { return false; }
        } else if g.len() < expected {
            return false;
        }
        let r = challenge(verifier, proofs, digest);
        let mut power = StepField::one();
        let mut combined = Vec::<CanonicalPasta<false>>::new();
        let mut commitments = Vec::with_capacity(proofs.len());
        let mut powers = Vec::with_capacity(proofs.len());
        for (index, proof) in proofs.iter().enumerate() {
            if proof.raw_bulletproof_challenges.len() != verifier.step_srs_length_log2 { return false; }
            let challenges: Vec<_> = proof.raw_bulletproof_challenges.iter()
                .map(|c| ScalarChallenge::new(*c).to_field(&verifier.step_endo)).collect();
            let mut coefficients = crate::canonical_coefficients::coefficients(&challenges);
            if coefficients.len() != expected { return false; }
            if index == 0 { combined = coefficients; }
            else {
                let rhs = CanonicalPastaRhs::new(CanonicalPasta::from_limbs(power.into_bigint().0).expect("power"));
                for (sum, coefficient) in combined.iter_mut().zip(&mut coefficients) {
                    coefficient.mul_prepared_assign(&rhs);
                    sum.add_assign(coefficient);
                }
            }
            commitments.push(proof.challenge_polynomial_commitment);
            powers.push(power);
            power *= r;
        }
        let lhs = match table {
            Some(table) => crate::canonical_msm::vesta_msm_fixed(table, &combined),
            None => crate::canonical_msm::vesta_msm_canonical_scalars(&g[..expected], &combined),
        }.into_affine();
        let rhs = <<Vesta as AffineRepr>::Group as VariableBaseMSM>::msm(&commitments, &powers)
            .expect("equal batch lengths").into_affine();
        lhs == rhs
    })();
    crate::track_cycles(b"cycle-tracker-report-end:pickles-accumulator\n");
    result
}
