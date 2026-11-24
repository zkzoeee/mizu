//! Out-of-circuit Pickles verifier.
//!
//! Layering:
//!
//!   * [`wire`] (std) — serde-deserializable form of the four OCaml-produced
//!     fixture files, plus parsing. Host-side only.
//!   * [`convert`] (std) — host-side conversion of the parsed wire data into
//!     the verifier types ([`Verifier::new`], [`OcamlProof::into_verifiable`]).
//!   * [`types`] (no_std) — the verifier types ([`Verifier`],
//!     [`VerifiableProof`]) consumed by the crate-root [`verify`] /
//!     [`verify_batch`] entry points. These run `no_std` + `alloc`, suitable
//!     for an SP1 guest.
//!
//! Primitives (Pasta fields/curves, Poseidon, kimchi `VerifierIndex` /
//! `ProverProof` serde, SRS/MSM, `batch_verify_with_rng`, the linearization
//! interpreter) come from upstream proof-systems crates; only the
//! pickles-specific glue is implemented here.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod canonical_msm;
mod fixed_srs;
mod fixed_pallas;
mod canonical_coefficients;
mod experimental_cip;
mod accumulator_batch;
mod vesta_glv;
mod vesta_spikes;
pub mod deferred;
pub mod attestation;
pub mod serialize;
mod setup_data;
pub mod statement;
pub mod types;

#[cfg(feature = "std")]
pub mod wire;

#[cfg(feature = "std")]
pub mod convert;

use alloc::vec::Vec;

use ark_ec::{AffineRepr, CurveGroup, VariableBaseMSM};
use mina_curves::pasta::Vesta;
use mina_poseidon::sponge::ScalarChallenge;
use poly_commitment::commitment::b_poly_coefficients;

use types::{StepField, VerifiableProof, Verifier};

#[cfg(target_os = "zkvm")]
fn track_cycles(command: &[u8]) {
    sp1_lib::io::write(1, command);
}

#[cfg(not(target_os = "zkvm"))]
fn track_cycles(_command: &[u8]) {}

/// Verify a single Pickles proof against its tag's [`Verifier`]. Deterministic
/// + `no_std`.
pub fn verify(verifier: &Verifier, proof: &VerifiableProof) -> bool {
    verify_batch(verifier, core::slice::from_ref(proof))
}

/// Verify a batch of proofs sharing one tag. Deterministic + `no_std`.
///
/// Three stages, AND-folded:
///   1. [`accumulator_check`] per proof.
///   2. Reconstruct each proof's wrap kimchi public input from its expanded
///      deferred values ([`deferred::expand_deferred`] +
///      [`deferred::wrap_public_input`]).
///   3. One amortized kimchi `batch_verify_with_rng` over the batch, seeded by
///      [`batching_rng`] (Fiat–Shamir-derived; see its doc for the soundness
///      argument).
pub fn verify_batch(verifier: &Verifier, proofs: &[VerifiableProof]) -> bool {
    verify_batch_with_index_digest(verifier, proofs, None)
}

pub(crate) fn verify_batch_with_index_digest(
    verifier: &Verifier,
    proofs: &[VerifiableProof],
    digest: Option<StepField>,
) -> bool {
    verify_batch_with_index_digest_and_vesta(verifier, proofs, digest, None, None, None)
}

// Only the immutable embedded verifier supplies a borrowed generator slice.
// Ordinary/runtime verifiers continue to use their owned SRS without overrides.
pub(crate) fn verify_batch_with_index_digest_and_vesta(
    verifier: &Verifier,
    proofs: &[VerifiableProof],
    digest: Option<StepField>,
    vesta_generators: Option<&[Vesta]>,
    fixed_vesta: Option<&fixed_srs::FixedVestaSrs<'_>>,
    fixed_pallas: Option<&fixed_pallas::FixedPallasSrs<'_>>,
) -> bool {
    use groupmap::GroupMap;
    use kimchi::verifier::{batch_verify_with_rng, batch_verify_with_rng_and_index_digests, Context};
    use mina_curves::pasta::{Pallas, PallasParameters};
    use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
    use mina_poseidon::pasta::FULL_ROUNDS;
    use mina_poseidon::sponge::{DefaultFqSponge, DefaultFrSponge};
    use poly_commitment::commitment::CommitmentCurve;
    use poly_commitment::ipa::OpeningProof;
    use rand_chacha::ChaCha20Rng;
    use types::WrapField;

    if proofs.is_empty() {
        return false;
    }

    let generators = vesta_generators.unwrap_or_else(|| verifier.vesta_srs.g.as_slice());
    if matches!(option_env!("MIZU_BATCH_ACCUMULATOR"), Some("1")) {
        if !proofs.iter().all(|p| statement::check_proof_binding(verifier, p))
            || !accumulator_batch::check(verifier, proofs, generators, fixed_vesta, digest) {
            return false;
        }
    } else if !proofs.iter().all(|p| statement::check_proof_binding(verifier, p)
            && if let Some(table) = fixed_vesta {
                accumulator_check_fixed(verifier, p, table)
            } else {
                accumulator_check_with_generators(verifier, p, generators)
            })
    {
        return false;
    }

    track_cycles(b"cycle-tracker-report-start:pickles-wrap-prepare\n");
    let mut pis: Vec<Vec<WrapField>> = Vec::with_capacity(proofs.len());
    for p in proofs {
        let Some(dv) = deferred::try_expand_deferred(verifier, p) else {
            track_cycles(b"cycle-tracker-report-end:pickles-wrap-prepare\n");
            return false;
        };
        pis.push(deferred::wrap_public_input(
            &dv,
            p.messages_for_next_step_proof_digest,
            p.messages_for_next_wrap_proof_digest,
        ));
    }

    let mut rng = batching_rng(proofs, &pis);
    let contexts: Vec<Context<FULL_ROUNDS, Pallas, OpeningProof<Pallas, FULL_ROUNDS>, _>> = proofs
        .iter()
        .zip(pis.iter())
        .map(|(p, pi)| Context {
            verifier_index: &verifier.wrap_vk,
            proof: &p.wrap_proof,
            public_input: pi.as_slice(),
        })
        .collect();
    track_cycles(b"cycle-tracker-report-end:pickles-wrap-prepare\n");

    let group_map = <Pallas as CommitmentCurve>::Map::setup();
    type WrapFqSponge = DefaultFqSponge<PallasParameters, PlonkSpongeConstantsKimchi, FULL_ROUNDS>;
    type WrapFrSponge = DefaultFrSponge<WrapField, PlonkSpongeConstantsKimchi, FULL_ROUNDS>;
    track_cycles(b"cycle-tracker-report-start:pickles-wrap-kimchi\n");
    let verified = if let Some(digest) = digest {
        let digests = alloc::vec![digest; contexts.len()];
        if let Some(table) = fixed_pallas {
            type Big = ark_ff::BigInt<4>;
            type Batch<'a> = poly_commitment::commitment::BatchEvaluationProof<'a,Pallas,WrapFqSponge,OpeningProof<Pallas,FULL_ROUNDS>,FULL_ROUNDS>;
            let msm = |prefix: &[Pallas], ks: &[Big], h: &Pallas, hk: &Big, tail: &[Pallas], tk: &[Big]| {
                fixed_pallas::msm(&verifier.wrap_vk.srs.g, table, prefix, ks, h, hk, tail, tk)
            };
            let opening = |srs: &poly_commitment::ipa::SRS<Pallas>, map: &<Pallas as CommitmentCurve>::Map,
                           batch: &mut [Batch<'_>], rng: &mut ChaCha20Rng| {
                srs.verify_with_msm(map,batch,rng,Some(&msm))
            };
            kimchi::verifier::batch_verify_with_rng_and_index_digests_and_opening_verifier::<
                FULL_ROUNDS,Pallas,WrapFqSponge,WrapFrSponge,OpeningProof<Pallas,FULL_ROUNDS>,ChaCha20Rng,
            >(&group_map,&contexts,&mut rng,&digests,&opening).is_ok()
        } else {
        batch_verify_with_rng_and_index_digests::<
            FULL_ROUNDS, Pallas, WrapFqSponge, WrapFrSponge,
            OpeningProof<Pallas, FULL_ROUNDS>, ChaCha20Rng,
        >(&group_map, &contexts, &mut rng, &digests).is_ok()
        }
    } else {
        batch_verify_with_rng::<
            FULL_ROUNDS, Pallas, WrapFqSponge, WrapFrSponge,
            OpeningProof<Pallas, FULL_ROUNDS>, ChaCha20Rng,
        >(&group_map, &contexts, &mut rng).is_ok()
    };
    track_cycles(b"cycle-tracker-report-end:pickles-wrap-kimchi\n");
    verified
}

/// Fiat–Shamir-derived `ChaCha20Rng` for kimchi's batched dlog check, seeded
/// by `blake2s(domain ‖ for each (proof, pi): postcard(proof) ‖ canonical(pi))`
/// with length prefixes.
///
/// Binding the random linear combination of the IPA verification equations to
/// the proof is what makes the batched check sound: a fixed/known seed would
/// let a prover forge an invalid proof whose specific combination vanishes.
/// `postcard` is used for the (serde-only) `ProverProof`; the public input
/// uses `CanonicalSerialize` (bare `Vec<Fq>` doesn't impl native serde
/// `Serialize`, only the proof's `serde_as` wrappers do).
fn batching_rng(
    proofs: &[VerifiableProof],
    pis: &[Vec<types::WrapField>],
) -> rand_chacha::ChaCha20Rng {
    use ark_serialize::CanonicalSerialize;
    use blake2::{Blake2s256, Digest};
    use rand_chacha::ChaCha20Rng;
    use rand_core::SeedableRng;

    let mut hasher = Blake2s256::new();
    hasher.update(b"pickles-verifier/batch-dlog-rng/v1");
    for (p, pi) in proofs.iter().zip(pis.iter()) {
        let proof_bytes = postcard::to_allocvec(&p.wrap_proof).expect("serialize wrap proof");
        hasher.update((proof_bytes.len() as u64).to_le_bytes());
        hasher.update(&proof_bytes);
        let mut pi_bytes = Vec::new();
        pi.serialize_compressed(&mut pi_bytes)
            .expect("serialize public input");
        hasher.update((pi_bytes.len() as u64).to_le_bytes());
        hasher.update(&pi_bytes);
    }
    ChaCha20Rng::from_seed(hasher.finalize().into())
}

/// Stage 2: the IPA accumulator check. The proof's
/// `challenge_polynomial_commitment` must equal `compute_sg(bulletproof_challenges)`,
/// the non-hiding MSM of the IPA challenge polynomial `b(X)`'s coefficients
/// against the Vesta SRS generators. Plain arkworks MSM rather than the
/// std-gated `SRS::commit_non_hiding`.
pub fn accumulator_check(verifier: &Verifier, proof: &VerifiableProof) -> bool {
    accumulator_check_with_generators(verifier, proof, &verifier.vesta_srs.g)
}

/// Experimental batch accumulator only; empty batches are rejected. Full
/// verification still needs statement binding and the wrap proof checks.
pub fn accumulator_check_batch(verifier: &Verifier, proofs: &[VerifiableProof]) -> bool {
    accumulator_batch::check(verifier, proofs, &verifier.vesta_srs.g, None, None)
}

// Keep this MSM separate from the large batch verifier, as in the r75 ELF.
// Merging it into the batch verifier regressed r76/r77's accumulator code.
#[inline(never)]
fn accumulator_check_with_generators(
    verifier: &Verifier,
    proof: &VerifiableProof,
    g: &[Vesta],
) -> bool {
    track_cycles(b"cycle-tracker-report-start:pickles-accumulator\n");
    let chals: Vec<StepField> = proof
        .raw_bulletproof_challenges
        .iter()
        .map(|c| ScalarChallenge::new(*c).to_field(&verifier.step_endo))
        .collect();
    let expected_len = 1usize << verifier.step_srs_length_log2;
    let canonical_coefficients = matches!(option_env!("MIZU_CANONICAL_ACCUMULATOR_COEFFICIENTS"), Some("1"))
        && matches!(option_env!("MIZU_CANONICAL_MSM"), Some("1"))
        && matches!(option_env!("MIZU_FUSE_VESTA_BASE_CONVERSION"), Some("1"))
        && matches!(option_env!("MIZU_VESTA_GLV"), Some("1"))
        && matches!(option_env!("MIZU_VESTA_POINT_PRECOMPILE"), Some("1"))
        && option_env!("MIZU_VESTA_PREPARED_LANES").is_none();
    if canonical_coefficients {
        let coeffs = canonical_coefficients::coefficients(&chals);
        if coeffs.len() != expected_len || g.len() < expected_len {
            track_cycles(b"cycle-tracker-report-end:pickles-accumulator\n");
            return false;
        }
        let computed_sg = canonical_msm::vesta_msm_canonical_scalars(&g[..expected_len], &coeffs).into_affine();
        let valid = computed_sg == proof.challenge_polynomial_commitment;
        track_cycles(b"cycle-tracker-report-end:pickles-accumulator\n");
        return valid;
    }
    let coeffs = b_poly_coefficients(&chals);
    if coeffs.len() != expected_len || g.len() < expected_len {
        track_cycles(b"cycle-tracker-report-end:pickles-accumulator\n");
        return false;
    }
    let computed_sg = if matches!(option_env!("MIZU_CANONICAL_MSM"), Some("1")) {
        canonical_msm::vesta_msm(&g[..expected_len], &coeffs).into_affine()
    } else {
        <<Vesta as AffineRepr>::Group as VariableBaseMSM>::msm(&g[..expected_len], &coeffs)
            .expect("compute_sg MSM")
            .into_affine()
    };
    let valid = computed_sg == proof.challenge_polynomial_commitment;
    track_cycles(b"cycle-tracker-report-end:pickles-accumulator\n");
    valid
}

#[inline(never)]
fn accumulator_check_fixed(
    verifier: &Verifier, proof: &VerifiableProof,
    table: &fixed_srs::FixedVestaSrs<'_>,
) -> bool {
    track_cycles(b"cycle-tracker-report-start:pickles-accumulator\n");
    let chals: Vec<StepField> = proof.raw_bulletproof_challenges.iter()
        .map(|c| ScalarChallenge::new(*c).to_field(&verifier.step_endo)).collect();
    let expected_len = 1usize << verifier.step_srs_length_log2;
    let coeffs = canonical_coefficients::coefficients(&chals);
    if coeffs.len() != expected_len || table.len() != expected_len {
        track_cycles(b"cycle-tracker-report-end:pickles-accumulator\n");
        return false;
    }
    let computed = canonical_msm::vesta_msm_fixed(table, &coeffs).into_affine();
    let valid = computed == proof.challenge_polynomial_commitment;
    track_cycles(b"cycle-tracker-report-end:pickles-accumulator\n");
    valid
}

// The tests use `std::fs` to read fixture JSONs + `std::sync::OnceLock` for
// the shared SRSes, so they only build under `--features std`. The no_std
// build covers serialize's pod-layout tests (`mod serialize::tests`) and the
// wire/convert sibling modules' tests already gated by `cfg(feature = "std")`.
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::types::{VestaSrs, WrapSrs, STEP_IPA_ROUNDS};
    use crate::wire::{
        parse_app_statement, parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk,
        OcamlProof,
    };
    use ark_ff::{AdditiveGroup, Field, PrimeField, Zero};
    use mina_curves::pasta::{Pallas, Vesta};
    use poly_commitment::precomputed_srs::get_srs;
    use std::sync::{Arc, OnceLock};

    /// Shared Vesta SRS for the stage-2 accumulator MSM (loaded once from
    /// `proof-systems/srs/vesta.srs`).
    fn vesta_srs() -> &'static Arc<VestaSrs> {
        static SRS: OnceLock<Arc<VestaSrs>> = OnceLock::new();
        SRS.get_or_init(|| Arc::new(get_srs::<Vesta>()))
    }

    /// Shared Pallas SRS for the wrap dlog check (loaded once from
    /// `proof-systems/srs/pallas.srs`).
    fn wrap_srs() -> &'static Arc<WrapSrs> {
        static SRS: OnceLock<Arc<WrapSrs>> = OnceLock::new();
        SRS.get_or_init(|| Arc::new(get_srs::<Pallas>()))
    }

    fn fixture(dir: &str, file: &str) -> String {
        let path = format!("{}/../../fixtures/{dir}/{file}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    /// Full end-to-end out-of-circuit verification over the fixture matrix.
    /// A `Verifier` is built per fixture (each has its own wrap VK); SRSes are
    /// shared across iterations via `OnceLock`.
    #[test]
    fn verify_accepts_fixtures() {
        for dir in [
            "mainnet-blockchain-snark",
            "ocaml-compat",
            "nrr",
            "simplechain/wrap0",
            "simplechain/wrap1",
            "simplechain/wrap2",
            "treeproofreturn/wrap0",
            "treeproofreturn/wrap1",
            "treeproofreturn/wrap2",
        ] {
            let ocaml =
                OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
            let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
            let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
            let stmt =
                parse_app_statement_fields(&fixture(dir, "app_statement.json")).expect("stmt");

            let vp = ocaml
                .into_verifiable(wrap_proof, &wrap_vk, &stmt)
                .expect("conversion");
            let verifier = Verifier::new(wrap_vk, wrap_srs().clone(), vesta_srs().clone(), 1);

            assert!(verify(&verifier, &vp), "verify should accept {dir}");
        }
    }

    #[test]
    fn verify_rejects_mutated_application_statement() {
        use ark_ff::One;

        let dir = "mainnet-blockchain-snark";
        let ocaml =
            OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
        let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
        let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
        let stmt = parse_app_statement(&fixture(dir, "app_statement.json")).expect("stmt");
        let mut proof = ocaml
            .into_verifiable(wrap_proof, &wrap_vk, &[stmt])
            .expect("conversion");
        let verifier = Verifier::new(wrap_vk, wrap_srs().clone(), vesta_srs().clone(), 1);

        proof.app_state[0] += StepField::one();

        assert!(
            !verify(&verifier, &proof),
            "application statement must be bound inside the verifier"
        );
    }

    fn mainnet_proof() -> (Verifier, VerifiableProof) {
        let dir = "mainnet-blockchain-snark";
        let ocaml =
            OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
        let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
        let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
        let stmt = parse_app_statement(&fixture(dir, "app_statement.json")).expect("stmt");
        let proof = ocaml
            .into_verifiable(wrap_proof, &wrap_vk, &[stmt])
            .expect("conversion");
        let verifier = Verifier::new(wrap_vk, wrap_srs().clone(), vesta_srs().clone(), 1);
        (verifier, proof)
    }

    #[test]
    fn experimental_batch_accepts_and_rejects_corruption_at_every_position() {
        let (verifier, proof) = mainnet_proof();
        assert!(!accumulator_check_batch(&verifier, &[]));
        for n in [1, 2, 4, 8] {
            assert!(accumulator_check_batch(&verifier, &vec![proof.clone(); n]));
        }
        for bad in 0..3 {
            let mut batch = vec![proof.clone(); 3];
            batch[bad].raw_bulletproof_challenges[0] += StepField::ONE;
            assert!(!accumulator_check_batch(&verifier, &batch));
            batch[bad] = proof.clone();
            batch[bad].challenge_polynomial_commitment = Vesta::generator();
            assert!(!accumulator_check_batch(&verifier, &batch));
        }
        let mut batch = vec![proof.clone(); 2];
        let delta = Vesta::generator();
        batch[0].challenge_polynomial_commitment = (proof.challenge_polynomial_commitment + delta).into_affine();
        batch[1].challenge_polynomial_commitment = (proof.challenge_polynomial_commitment - delta).into_affine();
        assert!(!accumulator_check_batch(&verifier, &batch), "opposite errors must not cancel");
        let challenge = accumulator_batch::challenge(&verifier, &batch, None);
        batch.swap(0, 1);
        assert_ne!(challenge, accumulator_batch::challenge(&verifier, &batch, None));
        assert!(!accumulator_check_batch(&verifier, &batch));
        let mut short = Verifier::new(verifier.wrap_vk.clone(), wrap_srs().clone(),
            Arc::new(VestaSrs::new(verifier.vesta_srs.g[..8].to_vec(), verifier.vesta_srs.h)), 1);
        assert!(!accumulator_check_batch(&short, &[proof.clone(), proof.clone()]));
        short.step_srs_length_log2 = usize::BITS as usize;
        assert!(!accumulator_check_batch(&short, &[proof.clone(), proof]));
    }

    /// Zakura's prepared zero-check rule: the combination challenge must be
    /// derived after every equation is fixed and must change with every input
    /// that defines one. Each case below alters exactly one such input.
    #[test]
    fn experimental_batch_challenge_binds_every_transcript_input() {
        let (verifier, proof) = mainnet_proof();
        let batch = vec![proof.clone(); 2];
        let base = accumulator_batch::challenge(&verifier, &batch, None);
        assert!(!base.is_zero(), "the zero challenge is excluded");

        let digest = StepField::from(7u64);
        assert_ne!(base, accumulator_batch::challenge(&verifier, &batch, Some(digest)));
        assert_ne!(
            accumulator_batch::challenge(&verifier, &batch, Some(digest)),
            accumulator_batch::challenge(&verifier, &batch, Some(digest + StepField::ONE)),
            "the verifier-key digest is bound"
        );

        let three = vec![proof.clone(); 3];
        assert_ne!(base, accumulator_batch::challenge(&verifier, &three, None), "count is bound");

        let mut changed = batch.clone();
        changed[1].raw_bulletproof_challenges[5] += StepField::ONE;
        assert_ne!(base, accumulator_batch::challenge(&verifier, &changed, None), "challenges are bound");

        let mut changed = batch.clone();
        changed[1].challenge_polynomial_commitment = Vesta::generator();
        assert_ne!(base, accumulator_batch::challenge(&verifier, &changed, None), "commitments are bound");

        let mut wider = Verifier::new(verifier.wrap_vk.clone(), wrap_srs().clone(), vesta_srs().clone(), 1);
        wider.step_srs_length_log2 += 1;
        assert_ne!(base, accumulator_batch::challenge(&wider, &batch, None), "SRS size is bound");

        // Deterministic: the same transcript yields the same challenge.
        assert_eq!(base, accumulator_batch::challenge(&verifier, &batch, None));
    }

    fn compat_proof() -> (Verifier, VerifiableProof) {
        let dir = "ocaml-compat";
        let ocaml =
            OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
        let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
        let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
        let stmt = parse_app_statement_fields(&fixture(dir, "app_statement.json"))
            .expect("application statement");
        let proof = ocaml
            .into_verifiable(wrap_proof, &wrap_vk, &stmt)
            .expect("conversion");
        let verifier = Verifier::new(wrap_vk, wrap_srs().clone(), vesta_srs().clone(), 1);
        (verifier, proof)
    }

    fn assert_serial_msm_matches_naive<C>(len: usize)
    where
        C: AffineRepr,
        C::Group: VariableBaseMSM<MulBase = C>,
        C::ScalarField: PrimeField,
    {
        let bases: Vec<C> = (0..len)
            .map(|i| (C::generator() * C::ScalarField::from((i as u64) + 2)).into_affine())
            .collect();
        let scalars: Vec<C::ScalarField> = (0..len)
            .map(|i| {
                let mut scalar = C::ScalarField::from((i as u64) + 11);
                for round in 0..4 {
                    scalar.square_in_place();
                    scalar += C::ScalarField::from((i + round + 3) as u64);
                }
                match i % 19 {
                    0 => C::ScalarField::ZERO,
                    1 => C::ScalarField::ONE,
                    _ => scalar,
                }
            })
            .collect();
        let bigints: Vec<_> = scalars.iter().map(|scalar| scalar.into_bigint()).collect();

        let serial =
            ark_ec::scalar_mul::variable_base::msm_bigint_serial::<C::Group>(&bases, &bigints);
        let naive = bases
            .iter()
            .zip(&scalars)
            .fold(C::Group::zero(), |mut sum, (base, scalar)| {
                sum += *base * scalar;
                sum
            });

        assert_eq!(serial, naive, "serial MSM mismatch at length {len}");
    }

    #[test]
    fn zkvm_serial_msm_matches_naive_on_both_pasta_curves() {
        for len in [0, 1, 2, 31, 32, 257] {
            assert_serial_msm_matches_naive::<Pallas>(len);
            assert_serial_msm_matches_naive::<Vesta>(len);
        }
    }

    #[test]
    fn zkvm_serial_msm_accepts_the_real_accumulator_commitment() {
        let (verifier, proof) = mainnet_proof();
        let challenges: Vec<StepField> = proof
            .raw_bulletproof_challenges
            .iter()
            .map(|challenge| ScalarChallenge::new(*challenge).to_field(&verifier.step_endo))
            .collect();
        let coefficients = b_poly_coefficients(&challenges);
        assert_eq!(coefficients.len(), 1usize << STEP_IPA_ROUNDS);
        assert!(verifier.vesta_srs.g.len() >= coefficients.len());
        let bigints: Vec<_> = coefficients
            .iter()
            .map(|coefficient| coefficient.into_bigint())
            .collect();

        let commitment = ark_ec::scalar_mul::variable_base::msm_bigint_serial::<
            <Vesta as AffineRepr>::Group,
        >(&verifier.vesta_srs.g[..coefficients.len()], &bigints)
        .into_affine();

        assert_eq!(commitment, proof.challenge_polynomial_commitment);
    }

    #[test]
    fn canonical_accumulator_coefficients_match_real_commitment_and_bind_values() {
        let (verifier, proof) = mainnet_proof();
        let challenges: Vec<StepField> = proof.raw_bulletproof_challenges.iter()
            .map(|c| ScalarChallenge::new(*c).to_field(&verifier.step_endo)).collect();
        let mut coeffs = crate::canonical_coefficients::coefficients(&challenges);
        let g = &verifier.vesta_srs.g[..coeffs.len()];
        let honest = crate::canonical_msm::vesta_msm_canonical_scalars(g, &coeffs).into_affine();
        assert_eq!(honest, proof.challenge_polynomial_commitment);
        coeffs[0].add_assign(&mina_curves::pasta::canonical_words::CanonicalPasta::from_limbs([1, 0, 0, 0]).unwrap());
        let changed = crate::canonical_msm::vesta_msm_canonical_scalars(g, &coeffs).into_affine();
        assert_ne!(changed, proof.challenge_polynomial_commitment);
        assert_eq!(changed, (honest + g[0]).into_affine(), "coefficient zero binds the first generator");
    }

    #[test]
    fn two_proof_batch_accepts_and_rejects_a_corrupted_member() {
        let (verifier, proof) = mainnet_proof();
        assert!(verify_batch(&verifier, &[proof.clone(), proof.clone()]));
        let mut corrupted = proof.clone();
        corrupted.wrap_proof.proof.z1 += types::WrapField::from(1u64);
        assert!(!verify_batch(&verifier, &[proof, corrupted]));
    }

    #[test]
    fn segmented_ipa_batch_fallback_accepts_honest_and_rejects_corruption() {
        let (verifier, proof) = mainnet_proof();
        // Deliberately too short: the segmented callback must rebuild the
        // complete MSM from its borrowed prefix plus h/tail, for both scalar
        // layouts (single canonical proof and multi-proof field accumulation).
        let bytes = crate::fixed_pallas::encode(&verifier.wrap_vk.srs.g[..2]);
        let table = crate::fixed_pallas::decode(&bytes, 2);
        let encoded_digest = crate::serialize::embedded_wrap_digest(&verifier.wrap_vk);
        let digest = types::StepField::from_bigint(ark_ff::BigInt(core::array::from_fn(|i|
            u64::from_le_bytes(encoded_digest[8*i..8*i+8].try_into().unwrap())))).unwrap();
        for count in [1, 2] {
            let mut proofs = vec![proof.clone(); count];
            assert!(verify_batch_with_index_digest_and_vesta(&verifier, &proofs,
                Some(digest), None, None, Some(&table)));
            proofs[count - 1].wrap_proof.proof.z1 += types::WrapField::from(1u64);
            assert!(!verify_batch_with_index_digest_and_vesta(&verifier, &proofs,
                Some(digest), None, None, Some(&table)));
        }
    }

    #[test]
    fn accumulator_rejects_a_truncated_step_srs() {
        let (mut verifier, proof) = mainnet_proof();
        let mut truncated = VestaSrs::default();
        truncated.g = verifier.vesta_srs.g[..(1usize << STEP_IPA_ROUNDS) - 1].to_vec().into();
        verifier.vesta_srs = Arc::new(truncated);

        assert!(!accumulator_check(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_deferred_plonk_challenge() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.raw_plonk.alpha += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_bulletproof_challenge() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.raw_bulletproof_challenges[0] += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_accumulator_commitment() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.challenge_polynomial_commitment.x += types::WrapField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_previous_proof_challenge() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.old_bulletproof_challenges[0][0] += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_previous_evaluation() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.prev_evals.ft_eval1 += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_feature_flag() {
        let (verifier, mut proof) = compat_proof();
        proof.raw_plonk.feature_flags.range_check0 = false;
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_joint_combiner() {
        use ark_ff::One;
        let (verifier, mut proof) = compat_proof();
        *proof
            .raw_plonk
            .joint_combiner
            .as_mut()
            .expect("fixture includes lookup parameters") += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_lookup_evaluation() {
        use ark_ff::One;
        let (verifier, mut proof) = compat_proof();
        proof
            .prev_evals
            .lookup_aggregation
            .as_mut()
            .expect("fixture includes lookup aggregation")
            .zeta[0] += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    /// Encode→decode round-trip: build a [`Verifier`] from the mainnet
    /// blockchain SNARK fixture, run it through
    /// [`serialize::encode_verifier_blob`] then [`serialize::decode_verifier_blob`],
    /// and confirm the decoded verifier (with a pod-cast SRS + pre-seeded
    /// wrap Lagrange basis) accepts the same proof. This is the path the SP1
    /// guest exercises.
    #[test]
    fn encode_decode_verifier_blob_round_trip_accepts() {
        let dir = "mainnet-blockchain-snark";
        let ocaml =
            OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
        let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
        let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
        let stmt = parse_app_statement(&fixture(dir, "app_statement.json")).expect("stmt");
        let vp = ocaml
            .into_verifiable(wrap_proof, &wrap_vk, &[stmt])
            .expect("conversion");

        // Encode at host side (basis is computed from the wrap SRS at wrap_vk's
        // domain), then decode (basis is seeded into the new SRS's cache).
        let blob = crate::serialize::encode_verifier_blob(
            vesta_srs(),
            wrap_srs(),
            /* step_num_chunks */ 1,
            &wrap_vk,
        );
        // The decoder needs 8-byte alignment, which `Vec<u8>` already provides
        // on this platform (the underlying allocator returns max-aligned
        // blocks). The SP1 guest gets it from a `#[repr(C, align(8))]` wrapper
        // around `include_bytes!`.
        assert_eq!(blob.as_ptr() as usize % 8, 0, "blob ptr must be 8-aligned");
        let decoded = crate::serialize::decode_verifier_blob(&blob);

        assert!(
            verify(&decoded, &vp),
            "decoded verifier should accept mainnet blockchain SNARK"
        );
    }
    #[test]
    fn embedded_index_digest_matches_runtime_and_rejects_corruption() {
        use crate::serialize::{decode_embedded_verifier, embedded_wrap_digest};
        use ark_ec::AffineRepr;
        for dir in ["mainnet-blockchain-snark", "nrr"] {
            let ocaml = OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).unwrap();
            let vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).unwrap();
            let proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).unwrap();
            let stmt = parse_app_statement_fields(&fixture(dir, "app_statement.json")).unwrap();
            let vp = ocaml.into_verifiable(proof, &vk, &stmt).unwrap();
            let digest = embedded_wrap_digest(&vk);
            let mut changed_vk = vk.clone();
            changed_vk.sigma_comm[0].chunks[0] =
                (changed_vk.sigma_comm[0].chunks[0] + Pallas::generator()).into_affine();
            let changed_digest = embedded_wrap_digest(&changed_vk);
            assert_ne!(
                digest, changed_digest,
                "digest must bind changed commitments"
            );
            let bytes = crate::serialize::encode_embedded_verifier_blob(vesta_srs(), wrap_srs(), 1, &vk);
            let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
            let digest: &'static [u8; 32] = Box::leak(Box::new(digest));
            let fixed = decode_embedded_verifier(bytes, digest);
            let runtime = crate::serialize::decode_verifier_blob(bytes);
            assert!(verify(&runtime, &vp), "runtime {dir}");
            assert!(fixed.verify(&vp), "embedded {dir}");
            let wrong = decode_embedded_verifier(bytes, Box::leak(Box::new(changed_digest)));
            assert!(!wrong.verify(&vp), "mismatched digest must reject {dir}");
            let mut corrupted = vp.clone();
            corrupted.wrap_proof.proof.z1 += types::WrapField::from(1u64);
            assert!(!verify(&runtime, &corrupted));
            assert!(!fixed.verify(&corrupted));
        }
    }

    #[test]
    fn fixed_vesta_table_all_entries_match_srs_and_bind_the_commitment() {
        use crate::serialize::{decode_embedded_verifier, embedded_wrap_digest};
        let (verifier, proof) = mainnet_proof();
        let bytes = crate::fixed_srs::encode(&verifier.vesta_srs.g);
        let table = crate::fixed_srs::decode(&bytes, verifier.vesta_srs.g.len());
        let shift_scalar = match table.lane_shift() {
            120 => ark_ff::BigInt([0, 1u64 << 56, 0, 0]),
            126 => ark_ff::BigInt([0, 1u64 << 62, 0, 0]),
            128 => ark_ff::BigInt([0, 0, 1, 0]),
            other => panic!("unexpected Vesta lane shift {other}"),
        };
        for (i, g) in verifier.vesta_srs.g.iter().enumerate() {
            assert_eq!(table.point::<mina_curves::pasta::VestaParameters>(0,i), *g);
            assert_eq!(table.point::<mina_curves::pasta::VestaParameters>(1,i),
                g.mul_bigint(shift_scalar).into_affine());
        }
        if matches!(option_env!("MIZU_FIXED_VESTA_SRS"), Some("1")) {
            let digest = Box::leak(Box::new(embedded_wrap_digest(&verifier.wrap_vk)));
            let mut blob = crate::serialize::encode_embedded_verifier_blob(
                &verifier.vesta_srs, &verifier.wrap_vk.srs, 1, &verifier.wrap_vk);
            let start = blob.windows(8).rposition(|x| x == crate::fixed_srs::MAGIC).unwrap();
            let changed = (verifier.vesta_srs.g[0] + Vesta::generator()).into_affine();
            let at = start + crate::fixed_srs::HEADER;
            for (i,word) in changed.x.into_bigint().0.into_iter().chain(changed.y.into_bigint().0).enumerate() {
                blob[at+8*i..at+8*i+8].copy_from_slice(&word.to_le_bytes());
            }
            let blob = Box::leak(blob.into_boxed_slice());
            assert!(!decode_embedded_verifier(blob, digest).verify(&proof), "changed table point must reject");
            assert!(verify(&crate::serialize::decode_verifier_blob(blob), &proof),
                "runtime verifier must still use its own original generators");
        }
    }

    #[test]
    fn embedded_pallas_borrows_exact_blob_prefix_and_runtime_outlives_its_input() {
        use crate::serialize::{decode_embedded_verifier,decode_embedded_verifier_for_proof,decode_verifier_blob,embedded_wrap_digest};
        let (verifier,proof)=mainnet_proof();
        let blob=crate::serialize::encode_embedded_verifier_blob(&verifier.vesta_srs,&verifier.wrap_vk.srs,1,&verifier.wrap_vk);
        let runtime={let input=blob.clone();let runtime=decode_verifier_blob(&input);assert!(!runtime.wrap_vk.srs.g.is_borrowed());runtime};
        // Input has been freed; retained generators, basis/cache and full
        // verification must remain usable on the ordinary runtime path.
        assert_eq!(runtime.wrap_vk.srs.g.as_slice(),verifier.wrap_vk.srs.g.as_slice());assert!(verify(&runtime,&proof));
        let bytes:&'static[u8]=Box::leak(blob.into_boxed_slice());
        let digest=Box::leak(Box::new(embedded_wrap_digest(&verifier.wrap_vk)));
        let vlen=u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
        let at=8+72*vlen;let plen=u64::from_le_bytes(bytes[at..at+8].try_into().unwrap()) as usize;
        let expected_ptr=bytes[at+8..].as_ptr();
        let full=decode_embedded_verifier(bytes,digest);let prefix=decode_embedded_verifier_for_proof(bytes,digest,&proof);
        let (full_g,_)=full.wrap_storage_for_test();let (short_g,_)=prefix.wrap_storage_for_test();
        assert_eq!(full_g.len(),plen);assert_eq!(short_g.len(),1<<proof.wrap_proof.proof.lr.len());
        assert_eq!(short_g,&full_g[..short_g.len()]);
        if matches!(option_env!("MIZU_BORROW_PALLAS_SRS"),Some("1")) {
            assert_eq!(full_g.as_ptr() as *const u8,expected_ptr);assert_eq!(short_g.as_ptr() as *const u8,expected_ptr);
        } else {assert_ne!(full_g.as_ptr() as *const u8,expected_ptr);assert_ne!(short_g.as_ptr() as *const u8,expected_ptr);}
        assert!(full.verify(&proof));assert!(prefix.verify(&proof));
        let mut changed=proof;changed.wrap_proof.proof.z1+=types::WrapField::from(1u64);
        assert!(!full.verify(&changed));assert!(!prefix.verify(&changed));assert!(!verify(&runtime,&changed));
    }

    #[test]
    fn embedded_vesta_storage_and_generator_binding() {
        use crate::serialize::{decode_embedded_verifier, embedded_wrap_digest};
        let dir = "mainnet-blockchain-snark";
        let ocaml = OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).unwrap();
        let vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).unwrap();
        let proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).unwrap();
        let stmt = parse_app_statement_fields(&fixture(dir, "app_statement.json")).unwrap();
        let vp = ocaml.into_verifiable(proof, &vk, &stmt).unwrap();
        let digest: &'static [u8; 32] = Box::leak(Box::new(embedded_wrap_digest(&vk)));
        let blob = crate::serialize::encode_embedded_verifier_blob(vesta_srs(), wrap_srs(), 1, &vk);
        let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
        let fixed = decode_embedded_verifier(blob, digest);
        let (borrowed, owned_len) = fixed.vesta_storage_for_test();
        let enabled = matches!(option_env!("MIZU_BORROW_VESTA_SRS"), Some("1"));
        let fixed_table = matches!(option_env!("MIZU_FIXED_VESTA_SRS"), Some("1"));
        if enabled && fixed_table {
            // Fixed Vesta table supersedes raw Montgomery Vesta SRS: neither copy nor scan it.
            assert!(borrowed.is_none());
            assert_eq!(owned_len, 0, "embedded fixed-Vesta path must not copy Vesta g");
        } else {
            assert_eq!(borrowed.is_some(), enabled);
            if let Some(g) = borrowed {
                assert_eq!(owned_len, 0, "embedded path must not copy Vesta g");
                assert_eq!(g.as_ptr() as usize, blob.as_ptr() as usize + 8);
                assert_eq!(g, vesta_srs().g.as_slice());
            } else {
                assert_eq!(owned_len, vesta_srs().g.len());
            }
        }
        let runtime = crate::serialize::decode_verifier_blob(blob);
        assert_eq!(runtime.vesta_srs.g, vesta_srs().g,
            "ordinary decoder must retain owned SRS semantics");
        let g = vesta_srs().g.as_slice();
        assert!(accumulator_check_with_generators(&runtime, &vp, g));
        assert!(!accumulator_check_with_generators(&runtime, &vp, &g[..g.len() - 1]));
        let mut changed = g.to_vec();
        // Coefficient zero is always one, so this changes the MSM by exactly G.
        changed[0] = (changed[0] + Vesta::generator()).into_affine();
        assert!(!accumulator_check_with_generators(&runtime, &vp, &changed));
        let mut bad_srs = VestaSrs::default();
        bad_srs.g = changed.into();
        let bad_blob = crate::serialize::encode_embedded_verifier_blob(&bad_srs, wrap_srs(), 1, &vk);
        let bad_fixed = decode_embedded_verifier(Box::leak(bad_blob.into_boxed_slice()), digest);
        assert!(!bad_fixed.verify(&vp), "embedded blob must bind its actual generators");
        let mut corrupted = vp;
        corrupted.challenge_polynomial_commitment =
            (corrupted.challenge_polynomial_commitment + Vesta::generator()).into_affine();
        assert!(!accumulator_check_with_generators(&runtime, &corrupted, g));
    }


    #[test]
    fn wrap_prefix_uses_proof_geometry_and_preserves_verification() {
        use crate::serialize::{decode_embedded_verifier, decode_embedded_verifier_for_proof, embedded_wrap_digest};
        for dir in ["mainnet-blockchain-snark", "nrr"] {
            let ocaml=OcamlProof::parse(&fixture(dir,"public_input_skeleton.json")).unwrap();
            let vk=parse_wrap_vk(&fixture(dir,"vk.serde.json")).unwrap();
            let proof=parse_wrap_proof(&fixture(dir,"proof.serde.json")).unwrap();
            let stmt=parse_app_statement_fields(&fixture(dir,"app_statement.json")).unwrap();
            let vp=ocaml.into_verifiable(proof,&vk,&stmt).unwrap();
            let bytes: &'static [u8]=Box::leak(crate::serialize::encode_embedded_verifier_blob(vesta_srs(),wrap_srs(),1,&vk).into_boxed_slice());
            let digest: &'static [u8;32]=Box::leak(Box::new(embedded_wrap_digest(&vk)));
            let full=decode_embedded_verifier(bytes,digest);
            let short=decode_embedded_verifier_for_proof(bytes,digest,&vp);
            let active=1usize<<vp.wrap_proof.proof.lr.len();
            let (full_g,full_max)=full.wrap_storage_for_test();
            let (short_g,short_max)=short.wrap_storage_for_test();
            assert_eq!(short_g,&full_g[..active]);
            assert_eq!(short_max,full_max,"VK transcript/chunk exponent must not change");
            assert_eq!(short_max,vk.max_poly_size);
            assert!(full.verify(&vp)); assert!(short.verify(&vp));
            let mut bad=vp.clone(); bad.wrap_proof.proof.z1+=types::WrapField::from(1u64);
            assert!(!full.verify(&bad)); assert!(!short.verify(&bad));
            if dir=="mainnet-blockchain-snark" {
                for rounds in [0,1,14,16] {
                    let mut changed=vp.clone();
                    changed.wrap_proof.proof.lr.resize(rounds,(Pallas::generator(),Pallas::generator()));
                    let prefix=decode_embedded_verifier_for_proof(bytes,digest,&changed);
                    assert_eq!(full.verify(&changed),prefix.verify(&changed),"rounds={rounds}");
                }
                let mut smaller=vp.clone(); smaller.wrap_proof.proof.lr.clear();
                let prefix=decode_embedded_verifier_for_proof(bytes,digest,&smaller);
                assert!(!prefix.verify(&vp),"larger opening must reject without truncation/panic");
                let mut oversized=vp.clone();
                oversized.wrap_proof.proof.lr.resize(usize::BITS as usize,(Pallas::generator(),Pallas::generator()));
                assert!(!full.verify(&oversized));
                assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
                    decode_embedded_verifier_for_proof(bytes,digest,&oversized))).is_err());
            }
        }
    }

    #[test]
    fn embedded_pallas_table_matches_every_generator_and_binds_verification() {
        use crate::serialize::{decode_embedded_verifier,embedded_wrap_digest};
        let (verifier,proof)=mainnet_proof();
        let g=&verifier.wrap_vk.srs.g;let n=g.len().div_ceil(2);
        let bytes=crate::fixed_pallas::encode(&g[..n]);
        let table=crate::fixed_pallas::decode(&bytes,n);
        let shift_scalar = match table.lane_shift() {
            128 => ark_ff::BigInt([0, 0, 1, 0]),
            130 => ark_ff::BigInt([0, 0, 4, 0]),
            other => panic!("unexpected Pallas lane shift {other}"),
        };
        for i in 0..n {
            assert_eq!(table.point::<mina_curves::pasta::PallasParameters>(0,i),g[i]);
            assert_eq!(table.point::<mina_curves::pasta::PallasParameters>(1,i),g[i].mul_bigint(shift_scalar).into_affine());
        }
        if matches!(option_env!("MIZU_FIXED_PALLAS_SRS"),Some("1")) {
            let digest=Box::leak(Box::new(embedded_wrap_digest(&verifier.wrap_vk)));
            let mut blob=crate::serialize::encode_embedded_verifier_blob(&verifier.vesta_srs,&verifier.wrap_vk.srs,1,&verifier.wrap_vk);
            let start=blob.windows(8).rposition(|b|b==crate::fixed_pallas::MAGIC).unwrap()+crate::fixed_pallas::HEADER;
            let changed=(g[0]+Pallas::generator()).into_affine();
            for (i,w) in changed.x.into_bigint().0.into_iter().chain(changed.y.into_bigint().0).enumerate() {blob[start+8*i..start+8*i+8].copy_from_slice(&w.to_le_bytes());}
            let blob=Box::leak(blob.into_boxed_slice());
            assert!(!decode_embedded_verifier(blob,digest).verify(&proof),"changed Pallas table point must reject");
            assert!(verify(&crate::serialize::decode_verifier_blob(blob),&proof),"runtime decoder must keep its own SRS");
        }
    }

    #[test]
    fn verify_rejects_unbound_wrap_digest_prev_challenges_branch_and_empty_batch() {
        use ark_ec::AffineRepr;
        use ark_ff::One;

        let (verifier, proof) = mainnet_proof();
        assert!(!verify_batch(&verifier, &[]), "empty batch must be rejected");
        assert!(statement::check_proof_binding(&verifier, &proof));

        // 1. Mutating messages_for_next_wrap_proof_digest breaks binding immediately
        //    (even before Kimchi verify — simulating a forged Wrap proof with a
        //    different step_sg_fake hashed into messages_for_next_wrap_proof_digest).
        let mut forged_wrap_digest = proof.clone();
        forged_wrap_digest.messages_for_next_wrap_proof_digest += types::WrapField::one();
        assert!(!statement::check_proof_binding(&verifier, &forged_wrap_digest));
        assert!(!verify(&verifier, &forged_wrap_digest));

        // 2. Mutating challenge_polynomial_commitment while keeping digest fixed breaks binding.
        let mut forged_cpc = proof.clone();
        forged_cpc.challenge_polynomial_commitment =
            (forged_cpc.challenge_polynomial_commitment + Vesta::generator()).into_affine();
        assert!(!statement::check_proof_binding(&verifier, &forged_cpc));
        assert!(!verify(&verifier, &forged_cpc));

        // 3. Infinity challenge_polynomial_commitment must be rejected.
        let mut inf_cpc = proof.clone();
        inf_cpc.challenge_polynomial_commitment = Vesta::zero();
        assert!(!statement::check_proof_binding(&verifier, &inf_cpc));

        // 4. Decoupling wrap_proof.prev_challenges[i].comm from prev_step_sgs must be rejected.
        let mut swapped_prev_comm = proof.clone();
        swapped_prev_comm.wrap_proof.prev_challenges[0].comm.chunks[0] =
            (swapped_prev_comm.wrap_proof.prev_challenges[0].comm.chunks[0] + Pallas::generator())
                .into_affine();
        assert!(!statement::check_proof_binding(&verifier, &swapped_prev_comm));
        assert!(!verify(&verifier, &swapped_prev_comm));

        // 5. Decoupling wrap_proof.prev_challenges[i].chals from messages_for_next_wrap_proof_digest.
        let mut tampered_prev_chals = proof.clone();
        tampered_prev_chals.wrap_proof.prev_challenges[0].chals[0] += types::WrapField::one();
        assert!(!statement::check_proof_binding(&verifier, &tampered_prev_chals));

        // 6. Mismatched or out-of-range step_domain_log2 must be rejected without shift overflow.
        let mut bad_log2 = proof.clone();
        bad_log2.step_domain_log2 = 15;
        assert!(!statement::check_proof_binding(&verifier, &bad_log2));
        assert!(!verify(&verifier, &bad_log2));

        let mut overflow_log2 = proof.clone();
        overflow_log2.step_domain_log2 = usize::BITS as usize;
        assert!(!statement::check_proof_binding(&verifier, &overflow_log2));
        assert!(!verify(&verifier, &overflow_log2));

        // 7. Invalid proofs_verified_mask [true, false] must be rejected.
        let mut bad_mask = proof.clone();
        bad_mask.branch_data.proofs_verified_mask = [true, false];
        assert!(!statement::check_proof_binding(&verifier, &bad_mask));
        assert!(!verify(&verifier, &bad_mask));

        // 8. Wrong wrap opening round count must be rejected.
        let mut bad_lr = proof.clone();
        bad_lr.wrap_proof.proof.lr.pop();
        assert!(!statement::check_proof_binding(&verifier, &bad_lr));
        assert!(!verify(&verifier, &bad_lr));

        // 9. Dummy padding slots on mpv < 2 fixtures (nrr, simplechain/wrap0) must be bound.
        for dir in ["nrr", "simplechain/wrap0"] {
            let ocaml =
                OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
            let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
            let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
            let stmt =
                parse_app_statement_fields(&fixture(dir, "app_statement.json")).expect("stmt");
            let vp = ocaml.into_verifiable(wrap_proof, &wrap_vk, &stmt).expect("conv");
            let v = Verifier::new(wrap_vk, wrap_srs().clone(), vesta_srs().clone(), 1);
            assert!(statement::check_proof_binding(&v, &vp), "{dir} honest");

            let mut bad_dummy = vp.clone();
            bad_dummy.wrap_proof.prev_challenges[0].chals[0] += types::WrapField::one();
            bad_dummy.messages_for_next_wrap_proof_digest =
                statement::wrap_messages_digest(&bad_dummy).expect("rehashed");
            assert!(
                !statement::check_proof_binding(&v, &bad_dummy),
                "{dir}: tampering dummy wrap challenge slot must fail even if digest is rehashed"
            );
        }
    }

}
