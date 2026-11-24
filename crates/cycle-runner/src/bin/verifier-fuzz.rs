//! Randomized mutation and adversarial fuzz for the out-of-circuit Pickles verifier.
//!
//! Four layers, all executed natively (no SP1 proving):
//!
//!   * V1 / V1b — 30 typed structural & cryptographic mutators covering
//!     statement fields, deferred challenges, evaluations, commitments, IPA
//!     responses, `step_domain_log2`, `branch_data`, `wrap_proof.prev_challenges`,
//!     `wrap_proof.proof.lr` length/overflow edges, infinity commitments,
//!     `ft_eval1`, `z_comm`/`t_comm`, and `raw_plonk.{joint_combiner, feature_flags}`.
//!   * V2 — Single-bit mutations of the bincode-serialized proof. Any bit flip
//!     that deserializes must be rejected cleanly (returning `false`, never
//!     panicking), unless it landed strictly inside `IGNORED_FIELDS`.
//!   * V3 — Coordinated / relational attacks that recompute message digests
//!     (`messages_for_next_wrap_proof_digest`, `messages_for_next_step_proof_digest`)
//!     alongside tampered commitments/challenges/dummy padding/domain log2, plus
//!     cross-fixture proof grafting and a cross-VK verification matrix.
//!   * V4 — Differential equivalence between `Verifier::new` and
//!     `decode_embedded_verifier` (with all build-time SRS/setup flags), plus
//!     `verify_batch` / `embedded.verify_batch` over empty, honest, and mixed
//!     honest+corrupt batches.

use anyhow::{anyhow, bail, ensure, Context, Result};
use ark_ec::AffineRepr;
use ark_ff::{PrimeField, Zero};
use bincode::Options;
use clap::Parser;
use mina_curves::pasta::{Pallas, Vesta};
use pickles_verifier::serialize::{
    decode_embedded_verifier, embedded_wrap_digest, encode_embedded_verifier_blob,
    EmbeddedVerifier,
};
use pickles_verifier::statement::{app_state_digest, proofs_verified_count, wrap_messages_digest};
use pickles_verifier::types::{StepField, VerifiableProof, Verifier, WrapField};
use pickles_verifier::wire::{
    parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof,
};
use poly_commitment::precomputed_srs::get_srs;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

/// Every fixture the verifier crate's own acceptance test covers.
const FIXTURES: &str = "mainnet-blockchain-snark,ocaml-compat,nrr,\
                        simplechain/wrap0,simplechain/wrap1,simplechain/wrap2,\
                        treeproofreturn/wrap0,treeproofreturn/wrap1,treeproofreturn/wrap2";

#[derive(Parser)]
#[command(about = "Mutate Pickles proofs at random and require the native verifier to reject")]
struct Args {
    #[arg(long, default_value = "fixtures")]
    fixtures_root: PathBuf,
    /// Comma-separated fixture directories under the root.
    #[arg(long, default_value = FIXTURES)]
    fixtures: String,
    /// Decimal or `0x`-prefixed seed for the mutation stream.
    #[arg(long, default_value = "0x5eed0001", value_parser = parse_seed)]
    seed: u64,
    /// Typed field mutations per fixture (V1 + V1b).
    #[arg(long, default_value_t = 64)]
    typed: usize,
    /// Single-bit mutations of the serialized proof per fixture (V2).
    #[arg(long, default_value_t = 64)]
    bytes: usize,
    /// Disable V4 differential checks against `EmbeddedVerifier` (faster smoke mode).
    #[arg(long, default_value_t = false)]
    skip_embedded: bool,
    #[arg(long)]
    output: PathBuf,
}

fn parse_seed(value: &str) -> std::result::Result<u64, String> {
    match value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => value.parse(),
    }
    .map_err(|_| "seed must be decimal or 0x-prefixed".to_string())
}

/// splitmix64: a dependency-free stream, so a seed replays a run exactly.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }

    fn bytes32(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for chunk in out.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        out
    }

    /// A nonzero element of either Pasta field.
    fn step(&mut self) -> StepField {
        let value = StepField::from_le_bytes_mod_order(&self.bytes32());
        if value.is_zero() {
            StepField::from(1u64)
        } else {
            value
        }
    }

    fn wrap(&mut self) -> WrapField {
        let value = WrapField::from_le_bytes_mod_order(&self.bytes32());
        if value.is_zero() {
            WrapField::from(1u64)
        } else {
            value
        }
    }
}

/// The number of typed mutators `typed_mutation` selects from (18 V1 + 12 V1b).
const TYPED_MUTATORS: usize = 30;

/// Fields of `VerifiableProof` that no part of the verifier reads, so a
/// mutation of them is expected to be accepted.
const IGNORED_FIELDS: [&str; 1] = ["p_eval0_chunks"];

/// Whether `target` names an ignored field.
fn is_ignored(target: &str) -> bool {
    IGNORED_FIELDS.iter().any(|field| target.starts_with(field))
}

/// Apply one typed mutation, or report that this fixture has no such field.
fn typed_mutation(which: usize, proof: &mut VerifiableProof, rng: &mut Rng) -> Option<String> {
    let label = match which {
        0 => {
            let i = rng.below(proof.app_state.len().max(1));
            *proof.app_state.get_mut(i)? += rng.step();
            format!("app_state[{i}]")
        }
        1 => {
            let i = rng.below(proof.raw_bulletproof_challenges.len());
            proof.raw_bulletproof_challenges[i] += rng.step();
            format!("raw_bulletproof_challenges[{i}]")
        }
        2 => {
            let i = rng.below(proof.old_bulletproof_challenges.len().max(1));
            let row = proof.old_bulletproof_challenges.get_mut(i)?;
            let j = rng.below(row.len());
            row[j] += rng.step();
            format!("old_bulletproof_challenges[{i}][{j}]")
        }
        3 => {
            let i = rng.below(proof.p_eval0_chunks.len().max(1));
            *proof.p_eval0_chunks.get_mut(i)? += rng.step();
            format!("p_eval0_chunks[{i}]")
        }
        4 => {
            proof.sponge_digest_before_evaluations += rng.step();
            "sponge_digest_before_evaluations".to_string()
        }
        5 => {
            proof.messages_for_next_step_proof_digest += rng.step();
            "messages_for_next_step_proof_digest".to_string()
        }
        6 => {
            proof.messages_for_next_wrap_proof_digest += rng.wrap();
            "messages_for_next_wrap_proof_digest".to_string()
        }
        7 => {
            proof.challenge_polynomial_commitment = -proof.challenge_polynomial_commitment;
            "challenge_polynomial_commitment negated".to_string()
        }
        8 => {
            let i = rng.below(proof.prev_step_sgs.len().max(1));
            let point = proof.prev_step_sgs.get_mut(i)?;
            *point = -*point;
            format!("prev_step_sgs[{i}] negated")
        }
        9 => {
            proof.wrap_proof.proof.z1 += rng.wrap();
            "wrap_proof.proof.z1".to_string()
        }
        10 => {
            proof.wrap_proof.proof.z2 += rng.wrap();
            "wrap_proof.proof.z2".to_string()
        }
        11 => {
            let i = rng.below(proof.wrap_proof.proof.lr.len().max(1));
            let pair = proof.wrap_proof.proof.lr.get_mut(i)?;
            if rng.next_u64().is_multiple_of(2) {
                pair.0 = -pair.0;
            } else {
                pair.1 = -pair.1;
            }
            format!("wrap_proof.proof.lr[{i}] negated")
        }
        12 => {
            proof.wrap_proof.proof.delta = -proof.wrap_proof.proof.delta;
            "wrap_proof.proof.delta negated".to_string()
        }
        13 => {
            proof.wrap_proof.proof.sg = -proof.wrap_proof.proof.sg;
            "wrap_proof.proof.sg negated".to_string()
        }
        14 => {
            let i = rng.below(proof.wrap_proof.commitments.w_comm.len());
            let chunk = proof.wrap_proof.commitments.w_comm[i].chunks.first_mut()?;
            *chunk = -*chunk;
            format!("wrap_proof.commitments.w_comm[{i}] negated")
        }
        15 => {
            let which = rng.below(4);
            match which {
                0 => proof.raw_plonk.alpha += rng.step(),
                1 => proof.raw_plonk.beta += rng.step(),
                2 => proof.raw_plonk.gamma += rng.step(),
                _ => proof.raw_plonk.zeta += rng.step(),
            }
            format!("raw_plonk field {which}")
        }
        16 => {
            proof.prev_evals.ft_eval1 += rng.step();
            "prev_evals.ft_eval1".to_string()
        }
        17 => {
            let i = rng.below(proof.prev_evals.w.len());
            let evals = &mut proof.prev_evals.w[i];
            let zeta = evals.zeta.first_mut()?;
            *zeta += rng.step();
            format!("prev_evals.w[{i}].zeta[0]")
        }
        // --- V1b: structural, binding, infinity, and shift-overflow mutators ---
        18 => {
            let orig = proof.step_domain_log2;
            let choices = [orig.wrapping_add(1), orig.wrapping_sub(1), 0, 33, 64];
            let pick = choices[rng.below(choices.len())];
            proof.step_domain_log2 = if pick == orig { orig + 1 } else { pick };
            format!("step_domain_log2={}", proof.step_domain_log2)
        }
        19 => {
            proof.branch_data.domain_log2 += rng.step();
            "branch_data.domain_log2".to_string()
        }
        20 => {
            let orig = proof.branch_data.proofs_verified_mask;
            let choices = [[true, false], [false, false], [false, true], [true, true]];
            let mut pick = choices[rng.below(choices.len())];
            if pick == orig {
                pick = [true, false];
            }
            proof.branch_data.proofs_verified_mask = pick;
            format!("branch_data.proofs_verified_mask={pick:?}")
        }
        21 => {
            let i = rng.below(proof.wrap_proof.prev_challenges.len().max(1));
            let pc = proof.wrap_proof.prev_challenges.get_mut(i)?;
            let j = rng.below(pc.chals.len().max(1));
            *pc.chals.get_mut(j)? += rng.wrap();
            format!("wrap_proof.prev_challenges[{i}].chals[{j}]")
        }
        22 => {
            let i = rng.below(proof.wrap_proof.prev_challenges.len().max(1));
            let pc = proof.wrap_proof.prev_challenges.get_mut(i)?;
            let chunk = pc.comm.chunks.first_mut()?;
            *chunk = -*chunk;
            format!("wrap_proof.prev_challenges[{i}].comm negated")
        }
        23 => {
            let mode = rng.below(5);
            match mode {
                0 => {
                    proof.wrap_proof.prev_challenges.pop()?;
                    "wrap_proof.prev_challenges popped".to_string()
                }
                1 => {
                    let dup = proof.wrap_proof.prev_challenges.first()?.clone();
                    proof.wrap_proof.prev_challenges.push(dup);
                    "wrap_proof.prev_challenges pushed".to_string()
                }
                2 => {
                    let i = rng.below(proof.wrap_proof.prev_challenges.len().max(1));
                    proof.wrap_proof.prev_challenges.get_mut(i)?.chals.pop()?;
                    format!("wrap_proof.prev_challenges[{i}].chals truncated")
                }
                3 => {
                    let i = rng.below(proof.wrap_proof.prev_challenges.len().max(1));
                    proof.wrap_proof.prev_challenges.get_mut(i)?.comm.chunks.clear();
                    format!("wrap_proof.prev_challenges[{i}].comm.chunks cleared")
                }
                _ => {
                    let i = rng.below(proof.wrap_proof.prev_challenges.len().max(1));
                    let chunk = proof
                        .wrap_proof
                        .prev_challenges
                        .get_mut(i)?
                        .comm
                        .chunks
                        .first_mut()?;
                    *chunk = Pallas::zero();
                    format!("wrap_proof.prev_challenges[{i}].comm infinity")
                }
            }
        }
        24 => {
            let mode = rng.below(4);
            let dummy = (Pallas::generator(), Pallas::generator());
            match mode {
                0 => {
                    let _ = proof.wrap_proof.proof.lr.pop()?;
                    "wrap_proof.proof.lr popped (14)".to_string()
                }
                1 => {
                    proof.wrap_proof.proof.lr.push(dummy);
                    "wrap_proof.proof.lr pushed (16)".to_string()
                }
                2 => {
                    proof.wrap_proof.proof.lr.clear();
                    "wrap_proof.proof.lr cleared (0)".to_string()
                }
                _ => {
                    proof.wrap_proof.proof.lr.resize(64, dummy);
                    "wrap_proof.proof.lr resized (64)".to_string()
                }
            }
        }
        25 => {
            proof.challenge_polynomial_commitment = Vesta::zero();
            "challenge_polynomial_commitment infinity".to_string()
        }
        26 => {
            let i = rng.below(proof.prev_step_sgs.len().max(1));
            let point = proof.prev_step_sgs.get_mut(i)?;
            *point = Pallas::zero();
            format!("prev_step_sgs[{i}] infinity")
        }
        27 => {
            proof.wrap_proof.ft_eval1 += rng.wrap();
            "wrap_proof.ft_eval1".to_string()
        }
        28 => {
            if rng.next_u64().is_multiple_of(2) {
                let chunk = proof.wrap_proof.commitments.z_comm.chunks.first_mut()?;
                *chunk = -*chunk;
                "wrap_proof.commitments.z_comm negated".to_string()
            } else {
                let chunk = proof.wrap_proof.commitments.t_comm.chunks.first_mut()?;
                *chunk = -*chunk;
                "wrap_proof.commitments.t_comm negated".to_string()
            }
        }
        29 => {
            if rng.next_u64().is_multiple_of(2) {
                match &mut proof.raw_plonk.joint_combiner {
                    Some(jc) => *jc += rng.step(),
                    None => proof.raw_plonk.joint_combiner = Some(rng.step()),
                }
                "raw_plonk.joint_combiner".to_string()
            } else {
                let ff = &mut proof.raw_plonk.feature_flags;
                match rng.below(6) {
                    0 => ff.range_check0 = !ff.range_check0,
                    1 => ff.range_check1 = !ff.range_check1,
                    2 => ff.foreign_field_add = !ff.foreign_field_add,
                    3 => ff.foreign_field_mul = !ff.foreign_field_mul,
                    4 => ff.xor = !ff.xor,
                    _ => ff.rot = !ff.rot,
                }
                "raw_plonk.feature_flags toggled".to_string()
            }
        }
        other => unreachable!("no typed mutator {other}"),
    };
    Some(label)
}

/// Run a verification closure, capturing any panic as `None`.
fn run_quietly<F: FnOnce() -> bool>(f: F) -> Option<bool> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let verdict = catch_unwind(AssertUnwindSafe(f));
    std::panic::set_hook(hook);
    verdict.ok()
}

fn verify_quietly(verifier: &Verifier, proof: &VerifiableProof) -> Option<bool> {
    run_quietly(|| pickles_verifier::verify(verifier, proof))
}

fn verify_embedded_quietly(embedded: &EmbeddedVerifier, proof: &VerifiableProof) -> Option<bool> {
    run_quietly(|| embedded.verify(proof))
}

fn verify_batch_quietly(verifier: &Verifier, proofs: &[VerifiableProof]) -> Option<bool> {
    run_quietly(|| pickles_verifier::verify_batch(verifier, proofs))
}

fn verify_embedded_batch_quietly(
    embedded: &EmbeddedVerifier,
    proofs: &[VerifiableProof],
) -> Option<bool> {
    run_quietly(|| embedded.verify_batch(proofs))
}

fn bincode_options() -> impl Options {
    // `bincode::serialize`'s encoding, with a ceiling so a mutated length
    // prefix cannot ask for an unbounded allocation.
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .with_limit(64 * 1024 * 1024)
}

struct LoadedFixture {
    name: String,
    vk_sha256: String,
    hashes: Value,
    proof: VerifiableProof,
    verifier: Verifier,
    embedded: Option<&'static EmbeddedVerifier>,
}

fn load_fixture(
    name: &str,
    directory: &Path,
    skip_embedded: bool,
    embedded_cache: &mut BTreeMap<String, &'static EmbeddedVerifier>,
) -> Result<LoadedFixture> {
    let read = |file: &str| -> Result<String> {
        let path = directory.join(file);
        fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
    };
    let vk_json = read("vk.serde.json")?;
    let proof_json = read("proof.serde.json")?;
    let skeleton_json = read("public_input_skeleton.json")?;
    let app_json = read("app_statement.json")?;
    let wrap_vk = parse_wrap_vk(&vk_json).map_err(|error| anyhow!("parse wrap VK: {error}"))?;
    let wrap_proof =
        parse_wrap_proof(&proof_json).map_err(|error| anyhow!("parse wrap proof: {error}"))?;
    let ocaml = OcamlProof::parse(&skeleton_json)
        .map_err(|error| anyhow!("parse public-input skeleton: {error}"))?;
    let app_statement = parse_app_statement_fields(&app_json)
        .map_err(|error| anyhow!("parse app statement: {error}"))?;
    let proof = ocaml
        .into_verifiable(wrap_proof, &wrap_vk, &app_statement)
        .map_err(|error| anyhow!("convert VerifiableProof: {error}"))?;
    let pallas_srs = Arc::new(get_srs::<Pallas>());
    let vesta_srs = Arc::new(get_srs::<Vesta>());
    let vk_sha256 = hex::encode(Sha256::digest(vk_json.as_bytes()));
    let embedded = if skip_embedded {
        None
    } else {
        let entry = embedded_cache.entry(vk_sha256.clone()).or_insert_with(|| {
            let blob = encode_embedded_verifier_blob(&vesta_srs, &pallas_srs, 1, &wrap_vk);
            let digest = embedded_wrap_digest(&wrap_vk);
            let blob_static: &'static [u8] = Box::leak(blob.into_boxed_slice());
            let digest_static: &'static [u8; 32] = Box::leak(Box::new(digest));
            Box::leak(Box::new(decode_embedded_verifier(blob_static, digest_static)))
        });
        Some(*entry)
    };
    let verifier = Verifier::new(wrap_vk, pallas_srs, vesta_srs, 1);
    let hashes = json!({
        "vk.serde.json": vk_sha256,
        "proof.serde.json": hex::encode(Sha256::digest(proof_json.as_bytes())),
        "public_input_skeleton.json": hex::encode(Sha256::digest(skeleton_json.as_bytes())),
        "app_statement.json": hex::encode(Sha256::digest(app_json.as_bytes())),
    });
    Ok(LoadedFixture {
        name: name.to_string(),
        vk_sha256,
        hashes,
        proof,
        verifier,
        embedded,
    })
}

/// Build coordinated / rehashed-digest adversarial proofs (V3) for one fixture.
fn coordinated_attacks(
    verifier: &Verifier,
    honest: &VerifiableProof,
    rng: &mut Rng,
) -> Vec<(String, VerifiableProof)> {
    let mut attacks = Vec::new();

    // 1. Negate challenge_polynomial_commitment AND rehash wrap message digest.
    {
        let mut p = honest.clone();
        p.challenge_polynomial_commitment = -p.challenge_polynomial_commitment;
        if let Some(d) = wrap_messages_digest(&p) {
            p.messages_for_next_wrap_proof_digest = d;
        }
        attacks.push(("v3_rehash_wrap_comm".to_string(), p));
    }

    // 2. Mutate a wrap prev_challenge scalar AND rehash wrap message digest.
    if let Some(pc) = honest.wrap_proof.prev_challenges.last() {
        if !pc.chals.is_empty() {
            let mut p = honest.clone();
            let idx = p.wrap_proof.prev_challenges.len() - 1;
            let j = rng.below(p.wrap_proof.prev_challenges[idx].chals.len());
            p.wrap_proof.prev_challenges[idx].chals[j] += rng.wrap();
            if let Some(d) = wrap_messages_digest(&p) {
                p.messages_for_next_wrap_proof_digest = d;
            }
            attacks.push((format!("v3_rehash_wrap_prev_chals[{idx}][{j}]"), p));
        }
    }

    // 3. On mpv < 2 fixtures, tamper a leading dummy padding slot AND rehash wrap digest.
    if let Some(mpv) = proofs_verified_count(honest.branch_data.proofs_verified_mask) {
        if mpv < 2 && !honest.wrap_proof.prev_challenges.is_empty() {
            let mut p = honest.clone();
            let j = rng.below(p.wrap_proof.prev_challenges[0].chals.len());
            p.wrap_proof.prev_challenges[0].chals[j] += rng.wrap();
            if let Some(d) = wrap_messages_digest(&p) {
                p.messages_for_next_wrap_proof_digest = d;
            }
            attacks.push((format!("v3_rehash_dummy_pad_slot[0][{j}]"), p));
        }
    }

    // 4. Mutate app_state AND rehash messages_for_next_step_proof_digest.
    if !honest.app_state.is_empty() {
        let mut p = honest.clone();
        let idx = rng.below(p.app_state.len());
        p.app_state[idx] += rng.step();
        if let Some(d) = app_state_digest(verifier, &p) {
            p.messages_for_next_step_proof_digest = d;
        }
        attacks.push((format!("v3_rehash_app_state[{idx}]"), p));
    }

    // 5. Negate prev_step_sgs[0], sync wrap_proof.prev_challenges[pad].comm, AND rehash step digest.
    if !honest.prev_step_sgs.is_empty() {
        let mut p = honest.clone();
        p.prev_step_sgs[0] = -p.prev_step_sgs[0];
        let pad = p.wrap_proof.prev_challenges.len() - p.prev_step_sgs.len();
        if let Some(chunk) = p.wrap_proof.prev_challenges[pad].comm.chunks.first_mut() {
            *chunk = p.prev_step_sgs[0];
        }
        if let Some(d) = app_state_digest(verifier, &p) {
            p.messages_for_next_step_proof_digest = d;
        }
        attacks.push(("v3_rehash_prev_step_sg_synced".to_string(), p));
    }

    // 6. Mutate old_bulletproof_challenges[0] AND rehash step digest.
    if !honest.old_bulletproof_challenges.is_empty() {
        let mut p = honest.clone();
        let j = rng.below(p.old_bulletproof_challenges[0].len());
        p.old_bulletproof_challenges[0][j] += rng.step();
        if let Some(d) = app_state_digest(verifier, &p) {
            p.messages_for_next_step_proof_digest = d;
        }
        attacks.push((format!("v3_rehash_old_bp_chals[0][{j}]"), p));
    }

    // 7. Synchronized domain_log2 increment (step_domain_log2 AND branch_data.domain_log2).
    {
        let mut p = honest.clone();
        p.step_domain_log2 = p.step_domain_log2.saturating_add(1).min(32);
        p.branch_data.domain_log2 = StepField::from(p.step_domain_log2 as u64);
        attacks.push(("v3_synced_domain_log2".to_string(), p));
    }

    attacks
}

/// Evaluate one mutated candidate against both `Verifier::new` and (if enabled)
/// `EmbeddedVerifier`, recording any verdict divergence.
fn verify_differential(
    verifier: &Verifier,
    embedded: Option<&EmbeddedVerifier>,
    proof: &VerifiableProof,
    diff_mismatches: &mut Vec<Value>,
    context: &str,
) -> Option<bool> {
    let v_ref = verify_quietly(verifier, proof);
    if let Some(emb) = embedded {
        let v_emb = verify_embedded_quietly(emb, proof);
        if v_ref != v_emb {
            diff_mismatches.push(json!({
                "context": context,
                "reference": v_ref,
                "embedded": v_emb,
            }));
        }
    }
    v_ref
}

fn main() -> Result<()> {
    let args = Args::parse();
    let started = Instant::now();
    let mut rng = Rng(args.seed);
    let mut embedded_cache = BTreeMap::new();

    let mut loaded = Vec::new();
    for name in args.fixtures.split(',').filter(|n| !n.is_empty()) {
        let directory = args.fixtures_root.join(name);
        loaded.push(load_fixture(
            name,
            &directory,
            args.skip_embedded,
            &mut embedded_cache,
        )?);
    }

    let mut fixtures = Vec::new();
    let mut accepted_total = 0usize;
    let mut panicked_total = 0usize;
    let mut ignored_total = 0usize;
    let mut typed_ignored = 0usize;
    let mut byte_ignored_total = 0usize;
    let mut diff_mismatches: Vec<Value> = Vec::new();

    for f in &loaded {
        let name = &f.name;
        let honest = &f.proof;
        let verifier = &f.verifier;
        let embedded = f.embedded;

        let honest_verdict = verify_differential(
            verifier,
            embedded,
            honest,
            &mut diff_mismatches,
            &format!("{name}:honest"),
        );
        ensure!(
            honest_verdict == Some(true),
            "{name}: the unmutated fixture does not verify ({honest_verdict:?})"
        );

        let honest_bytes = bincode_options()
            .serialize(honest)
            .context("serialize the honest VerifiableProof")?;
        let round_trip: VerifiableProof = bincode_options()
            .deserialize(&honest_bytes)
            .context("the honest proof does not survive a bincode round trip")?;
        ensure!(
            verify_differential(
                verifier,
                embedded,
                &round_trip,
                &mut diff_mismatches,
                &format!("{name}:roundtrip"),
            ) == Some(true),
            "{name}: the round-tripped fixture does not verify"
        );

        // V1 + V1b: typed field & structural mutations.
        let mut rejected = 0usize;
        let mut panicked = Vec::new();
        let mut accepted = Vec::new();
        let mut not_applicable = Vec::new();
        let mut ignored = 0usize;
        let mut targets: BTreeMap<String, usize> = Default::default();
        let typed_count = args.typed.max(TYPED_MUTATORS);
        for i in 0..typed_count {
            let which = if i < TYPED_MUTATORS {
                i
            } else {
                rng.below(TYPED_MUTATORS)
            };
            let mut mutated = honest.clone();
            let Some(label) = typed_mutation(which, &mut mutated, &mut rng) else {
                not_applicable.push(which);
                continue;
            };
            *targets.entry(label.clone()).or_default() += 1;
            let ctx = format!("{name}:v1:{which}:{label}");
            match verify_differential(verifier, embedded, &mutated, &mut diff_mismatches, &ctx) {
                Some(false) => rejected += 1,
                Some(true) if is_ignored(&label) => ignored += 1,
                Some(true) => accepted.push(json!({"mutator": which, "target": label})),
                None => panicked.push(json!({"mutator": which, "target": label})),
            }
        }
        ignored_total += ignored;
        accepted_total += accepted.len();
        panicked_total += panicked.len();
        typed_ignored += ignored;

        // V2: single-bit mutations of the serialized proof.
        let mut decoded = 0usize;
        let mut undecodable = 0usize;
        let mut byte_rejected = 0usize;
        let mut byte_panicked = Vec::new();
        let mut byte_ignored = 0usize;
        let mut byte_accepted = Vec::new();
        for _ in 0..args.bytes {
            let mut bytes = honest_bytes.clone();
            let index = rng.below(bytes.len());
            let bit = rng.below(8);
            bytes[index] ^= 1 << bit;
            let Ok(mutated) = bincode_options().deserialize::<VerifiableProof>(&bytes) else {
                undecodable += 1;
                continue;
            };
            decoded += 1;
            let ctx = format!("{name}:v2:byte={index}:bit={bit}");
            match verify_differential(verifier, embedded, &mutated, &mut diff_mismatches, &ctx) {
                Some(false) => byte_rejected += 1,
                None => byte_panicked.push(json!({"byte": index, "bit": bit})),
                Some(true) => {
                    let mut restored = mutated.clone();
                    restored.p_eval0_chunks = honest.p_eval0_chunks.clone();
                    let only_ignored = bincode_options()
                        .serialize(&restored)
                        .map(|bytes| bytes == honest_bytes)
                        .unwrap_or(false);
                    if only_ignored {
                        byte_ignored += 1;
                    } else {
                        byte_accepted.push(json!({"byte": index, "bit": bit}));
                    }
                }
            }
        }
        accepted_total += byte_accepted.len();
        panicked_total += byte_panicked.len();
        ignored_total += byte_ignored;
        byte_ignored_total += byte_ignored;

        // V3: coordinated / rehashed-digest attacks.
        let mut v3_rejected = 0usize;
        let mut v3_accepted = Vec::new();
        let mut v3_panicked = Vec::new();
        for (label, attacked) in coordinated_attacks(verifier, honest, &mut rng) {
            let ctx = format!("{name}:{label}");
            match verify_differential(verifier, embedded, &attacked, &mut diff_mismatches, &ctx) {
                Some(false) => v3_rejected += 1,
                Some(true) => v3_accepted.push(json!({"attack": label})),
                None => v3_panicked.push(json!({"attack": label})),
            }
        }
        accepted_total += v3_accepted.len();
        panicked_total += v3_panicked.len();

        // V4: batch verification (empty, honest, and mixed honest+corrupt batches).
        let mut v4_checks = 0usize;
        let mut v4_failures = Vec::new();
        let check_batch = |label: &str,
                           batch: &[VerifiableProof],
                           expect: bool,
                           checks: &mut usize,
                           failures: &mut Vec<Value>,
                           panics: &mut usize,
                           diffs: &mut Vec<Value>| {
            *checks += 1;
            let v_ref = verify_batch_quietly(verifier, batch);
            if let Some(emb) = embedded {
                let v_emb = verify_embedded_batch_quietly(emb, batch);
                if v_ref != v_emb {
                    diffs.push(json!({
                        "context": format!("{name}:v4:{label}"),
                        "reference": v_ref,
                        "embedded": v_emb,
                    }));
                }
            }
            match v_ref {
                Some(v) if v == expect => {}
                Some(v) => failures.push(json!({"case": label, "expected": expect, "got": v})),
                None => {
                    *panics += 1;
                    failures.push(json!({"case": label, "expected": expect, "got": "panic"}));
                }
            }
        };

        check_batch(
            "empty_batch",
            &[],
            false,
            &mut v4_checks,
            &mut v4_failures,
            &mut panicked_total,
            &mut diff_mismatches,
        );
        let honest_pair = [honest.clone(), honest.clone()];
        check_batch(
            "honest_batch_2",
            &honest_pair,
            true,
            &mut v4_checks,
            &mut v4_failures,
            &mut panicked_total,
            &mut diff_mismatches,
        );
        // Corrupt at first, middle, and last positions of a 3-proof batch.
        for pos in 0..3usize {
            let mut batch = vec![honest.clone(), honest.clone(), honest.clone()];
            batch[pos].challenge_polynomial_commitment =
                -batch[pos].challenge_polynomial_commitment;
            if let Some(d) = wrap_messages_digest(&batch[pos]) {
                batch[pos].messages_for_next_wrap_proof_digest = d;
            }
            check_batch(
                &format!("corrupt_batch_3_pos_{pos}"),
                &batch,
                false,
                &mut v4_checks,
                &mut v4_failures,
                &mut panicked_total,
                &mut diff_mismatches,
            );
        }
        accepted_total += v4_failures.len();

        fixtures.push(json!({
            "fixture": name,
            "sha256": f.hashes,
            "honest_accepted": true,
            "serialized_bytes": honest_bytes.len(),
            "v1_typed": {
                "mutations": typed_count,
                "rejected": rejected,
                "accepted_in_an_ignored_field": ignored,
                "verifier_panicked": panicked,
                "accepted": accepted,
                "not_applicable_mutators": not_applicable,
                "targets": targets,
            },
            "v2_single_bit": {
                "mutations": args.bytes,
                "did_not_deserialize": undecodable,
                "deserialized": decoded,
                "rejected": byte_rejected,
                "accepted_in_an_ignored_field": byte_ignored,
                "verifier_panicked": byte_panicked,
                "accepted": byte_accepted,
            },
            "v3_coordinated": {
                "rejected": v3_rejected,
                "accepted": v3_accepted,
                "verifier_panicked": v3_panicked,
            },
            "v4_batch": {
                "checks": v4_checks,
                "failures": v4_failures,
            },
        }));
    }

    // V3 cross-fixture: cross-VK matrix + proof grafting between distinct fixtures.
    let mut cross_vk_rejected = 0usize;
    let mut cross_vk_accepted = Vec::new();
    let mut cross_vk_panicked = Vec::new();
    let mut graft_rejected = 0usize;
    let mut graft_accepted = Vec::new();
    let mut graft_panicked = Vec::new();

    for (i, a) in loaded.iter().enumerate() {
        for (j, b) in loaded.iter().enumerate() {
            if i == j {
                continue;
            }
            if a.vk_sha256 != b.vk_sha256 {
                let ctx = format!("v3_cross_vk:proof={}:vk={}", a.name, b.name);
                match verify_differential(
                    &b.verifier,
                    b.embedded,
                    &a.proof,
                    &mut diff_mismatches,
                    &ctx,
                ) {
                    Some(false) => cross_vk_rejected += 1,
                    Some(true) => {
                        cross_vk_accepted.push(json!({"proof": a.name, "vk": b.name}))
                    }
                    None => cross_vk_panicked.push(json!({"proof": a.name, "vk": b.name})),
                }
            } else {
                // Same VK (e.g. simplechain/wrap0..2, treeproofreturn/wrap0..2):
                // splice wrap_proof from `b` onto statement/accumulator from `a`
                // and rehash both message digests.
                let mut spliced = a.proof.clone();
                spliced.wrap_proof = b.proof.wrap_proof.clone();
                if let Some(d) = wrap_messages_digest(&spliced) {
                    spliced.messages_for_next_wrap_proof_digest = d;
                }
                if let Some(d) = app_state_digest(&a.verifier, &spliced) {
                    spliced.messages_for_next_step_proof_digest = d;
                }
                let ctx = format!("v3_graft:stmt={}:wrap={}", a.name, b.name);
                match verify_differential(
                    &a.verifier,
                    a.embedded,
                    &spliced,
                    &mut diff_mismatches,
                    &ctx,
                ) {
                    Some(false) => graft_rejected += 1,
                    Some(true) => {
                        graft_accepted.push(json!({"stmt": a.name, "wrap": b.name}))
                    }
                    None => graft_panicked.push(json!({"stmt": a.name, "wrap": b.name})),
                }
            }
        }
    }
    accepted_total += cross_vk_accepted.len() + graft_accepted.len();
    panicked_total += cross_vk_panicked.len() + graft_panicked.len();

    let report = json!({
        "tool": "verifier-mutation-fuzz",
        "seed": format!("{:#x}", args.seed),
        "typed_mutators": TYPED_MUTATORS,
        "ignored_fields": IGNORED_FIELDS,
        "embedded_differential": !args.skip_embedded,
        "proved": false,
        "scope": "native out-of-circuit verifier (Verifier::new + EmbeddedVerifier + verify_batch); no SP1 proof is generated",
        "fixtures": fixtures,
        "v3_cross_fixture": {
            "cross_vk_rejected": cross_vk_rejected,
            "cross_vk_accepted": cross_vk_accepted,
            "cross_vk_panicked": cross_vk_panicked,
            "graft_rejected": graft_rejected,
            "graft_accepted": graft_accepted,
            "graft_panicked": graft_panicked,
        },
        "v4_differential_mismatches": diff_mismatches,
        "accepted_mutations": accepted_total,
        "panicked_mutations": panicked_total,
        "accepted_in_an_ignored_field": ignored_total,
        "accepted_in_an_ignored_field_typed": typed_ignored,
        "accepted_in_an_ignored_field_single_bit": byte_ignored_total,
        "seconds": started.elapsed().as_secs_f64(),
    });
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !diff_mismatches.is_empty() {
        bail!(
            "{} differential mismatches between Verifier::new and EmbeddedVerifier",
            diff_mismatches.len()
        );
    }
    if accepted_total > 0 {
        bail!("{accepted_total} mutated proofs were accepted by the verifier");
    }
    if panicked_total > 0 {
        bail!("{panicked_total} mutated proofs panicked instead of returning false");
    }
    ensure!(
        ignored_total > 0,
        "no mutation of an ignored field was accepted, so the expectation that {IGNORED_FIELDS:?} \
         is read by nothing was not exercised"
    );
    Ok(())
}
