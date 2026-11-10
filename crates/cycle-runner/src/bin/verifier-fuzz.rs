//! Randomized mutation fuzz for the out-of-circuit Pickles verifier.
//!
//! The dev loop carries six hand-named proof corruptions and the verifier
//! crate a dozen fixed mutation tests, each of which changes one chosen field
//! and requires `pickles_verifier::verify` to reject. Native verification of a
//! fixture takes well under a second, so the same question can be asked
//! thousands of times with the field and the value drawn at random, over every
//! fixture rather than one.
//!
//! Two layers:
//!
//!   * V1 mutates one typed field of the `VerifiableProof` — a statement
//!     field, a deferred challenge, an evaluation, a commitment, an IPA
//!     response — with a random field element or a negated point, drawn from
//!     a seeded stream.
//!   * V2 flips one random bit of the serialized proof. Most flips fail to
//!     deserialize, which is reported separately; every one that deserializes
//!     must be rejected.
//!
//! Neither layer generates or verifies an SP1 proof: this is the native
//! verifier only, which is what makes it cheap enough to randomize.

use anyhow::{anyhow, bail, ensure, Context, Result};
use ark_ff::{PrimeField, Zero};
use bincode::Options;
use clap::Parser;
use mina_curves::pasta::{Pallas, Vesta};
use pickles_verifier::types::{StepField, VerifiableProof, Verifier, WrapField};
use pickles_verifier::wire::{
    parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof,
};
use poly_commitment::precomputed_srs::get_srs;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
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
    /// Typed field mutations per fixture.
    #[arg(long, default_value_t = 64)]
    typed: usize,
    /// Single-bit mutations of the serialized proof per fixture.
    #[arg(long, default_value_t = 64)]
    bytes: usize,
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

/// The number of typed mutators `typed_mutation` selects from.
const TYPED_MUTATORS: usize = 18;

/// Fields of `VerifiableProof` that no part of the verifier reads, so a
/// mutation of them is expected to be accepted.
///
/// `p_eval0_chunks` is a copy of `prev_evals.public_evals.zeta` that the host
/// conversion derives from it (`wire.rs`). The verifier works from the
/// authoritative copy inside `prev_evals`, and nothing in the verifier, the
/// guest, the attestation or the statement digest reads this one, so it is
/// dead weight in the guest's input rather than an unbound value: the proof's
/// public-input evaluation is still constrained, through the field that is
/// actually used. This fuzz run is what established that, and it fails if any
/// other field turns out to be ignored, or if this one stops being.
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
            proof.challenge_polynomial_commitment =
                -proof.challenge_polynomial_commitment;
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
        other => unreachable!("no typed mutator {other}"),
    };
    Some(label)
}

/// `verify`, with a panic reported rather than propagated: a verifier that
/// aborts on a malformed proof rejects it for liveness but not by returning
/// false, and the two outcomes are counted apart.
fn verify_quietly(verifier: &Verifier, proof: &VerifiableProof) -> Option<bool> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let verdict = catch_unwind(AssertUnwindSafe(|| pickles_verifier::verify(verifier, proof)));
    std::panic::set_hook(hook);
    verdict.ok()
}

fn bincode_options() -> impl Options {
    // `bincode::serialize`'s encoding, with a ceiling so a mutated length
    // prefix cannot ask for an unbounded allocation.
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .with_limit(64 * 1024 * 1024)
}

fn load_fixture(directory: &Path) -> Result<(VerifiableProof, Verifier, Value)> {
    let read = |name: &str| -> Result<String> {
        let path = directory.join(name);
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
    let verifier =
        Verifier::new(wrap_vk, Arc::new(get_srs::<Pallas>()), Arc::new(get_srs::<Vesta>()), 1);
    let hashes = json!({
        "vk.serde.json": hex::encode(Sha256::digest(vk_json.as_bytes())),
        "proof.serde.json": hex::encode(Sha256::digest(proof_json.as_bytes())),
        "public_input_skeleton.json": hex::encode(Sha256::digest(skeleton_json.as_bytes())),
        "app_statement.json": hex::encode(Sha256::digest(app_json.as_bytes())),
    });
    Ok((proof, verifier, hashes))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let started = Instant::now();
    let mut rng = Rng(args.seed);
    let mut fixtures = Vec::new();
    let mut accepted_total = 0usize;
    let mut ignored_total = 0usize;
    let mut typed_ignored = 0usize;
    let mut byte_ignored_total = 0usize;
    for name in args.fixtures.split(',').filter(|n| !n.is_empty()) {
        let directory = args.fixtures_root.join(name);
        let (honest, verifier, hashes) = load_fixture(&directory)?;
        let honest_verdict = verify_quietly(&verifier, &honest);
        ensure!(
            honest_verdict == Some(true),
            "{name}: the unmutated fixture does not verify ({honest_verdict:?})"
        );
        let honest_bytes = bincode_options()
            .serialize(&honest)
            .context("serialize the honest VerifiableProof")?;
        let round_trip: VerifiableProof = bincode_options()
            .deserialize(&honest_bytes)
            .context("the honest proof does not survive a bincode round trip")?;
        ensure!(
            verify_quietly(&verifier, &round_trip) == Some(true),
            "{name}: the round-tripped fixture does not verify"
        );

        // V1: typed field mutations.
        let mut rejected = 0usize;
        let mut panicked = Vec::new();
        let mut accepted = Vec::new();
        let mut not_applicable = Vec::new();
        let mut ignored = 0usize;
        let mut targets: std::collections::BTreeMap<String, usize> = Default::default();
        // Every mutator once, with random values, then random draws: a
        // mutator that is never drawn tests nothing, and the expectation that
        // one field is ignored has to be exercised to mean anything.
        for i in 0..args.typed {
            let which = if i < TYPED_MUTATORS { i } else { rng.below(TYPED_MUTATORS) };
            let mut mutated = honest.clone();
            let Some(label) = typed_mutation(which, &mut mutated, &mut rng) else {
                not_applicable.push(which);
                continue;
            };
            *targets.entry(label.clone()).or_default() += 1;
            match verify_quietly(&verifier, &mutated) {
                Some(false) => rejected += 1,
                Some(true) if is_ignored(&label) => ignored += 1,
                Some(true) => accepted.push(json!({"mutator": which, "target": label})),
                None => panicked.push(json!({"mutator": which, "target": label})),
            }
        }
        ignored_total += ignored;
        accepted_total += accepted.len();
        typed_ignored += ignored;

        // V2: single-bit mutations of the serialized proof.
        let mut decoded = 0usize;
        let mut undecodable = 0usize;
        let mut byte_rejected = 0usize;
        let mut byte_panicked = 0usize;
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
            match verify_quietly(&verifier, &mutated) {
                Some(false) => byte_rejected += 1,
                None => byte_panicked += 1,
                // An accepted flip has to be one that reached nothing the
                // verifier reads. Restoring the ignored fields and
                // re-serializing settles it: if the bytes are the honest ones
                // again, the flip touched only those fields.
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
        ignored_total += byte_ignored;
        byte_ignored_total += byte_ignored;

        fixtures.push(json!({
            "fixture": name,
            "sha256": hashes,
            "honest_accepted": true,
            "serialized_bytes": honest_bytes.len(),
            "v1_typed": {
                "mutations": args.typed,
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
        }));
    }
    let report = json!({
        "tool": "verifier-mutation-fuzz",
        "seed": format!("{:#x}", args.seed),
        "typed_mutators": TYPED_MUTATORS,
        "ignored_fields": IGNORED_FIELDS,
        "proved": false,
        "scope": "the native out-of-circuit verifier only; no SP1 proof is generated or verified",
        "note": "a mutation the verifier panics on is rejected for liveness but not by \
                 returning false, so the two outcomes are counted apart. A mutation of a field \
                 no part of the verifier reads is expected to be accepted and counted apart as \
                 well; any other accepted mutation is a failure",
        "fixtures": fixtures,
        "accepted_mutations": accepted_total,
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
    if accepted_total > 0 {
        bail!("{accepted_total} mutated proofs were accepted by the verifier");
    }
    ensure!(
        ignored_total > 0,
        "no mutation of an ignored field was accepted, so the expectation that {IGNORED_FIELDS:?} \
         is read by nothing was not exercised"
    );
    Ok(())
}
