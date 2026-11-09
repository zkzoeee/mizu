//! Serialize a fixture for CPU trace/proof tools; never execute or prove.
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use pickles_verifier::types::VerifiableProof;
use pickles_verifier::wire::{parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::{Path, PathBuf}};
#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "fixtures/mainnet-blockchain-snark")]
    fixture_dir: PathBuf,
    #[arg(long)]
    output_dir: PathBuf,
}
fn hash(bytes: &[u8]) -> String { hex::encode(Sha256::digest(bytes)) }

fn main() -> Result<()> {
    let args = Args::parse();
    let (proof, _, hashes) = load_fixture(&args.fixture_dir)?;
    let input = bincode::serialize(&proof)?;
    fs::create_dir_all(&args.output_dir)?;
    fs::write(args.output_dir.join("proof-input.bin"), &input)?;
    // Only the optional settlement guest consumes this second input.
    fs::write(args.output_dir.join("receipt-input.bin"), [0])?;
    fs::write(args.output_dir.join("inputs.json"), serde_json::to_vec_pretty(&json!({
        "fixture_sha256": hashes, "proof_input_sha256": hex::encode(Sha256::digest(&input)),
        "receipt_input_sha256": hex::encode(Sha256::digest([0])), "proof_generated": false
    }))?)?;
    Ok(())
}
fn load_fixture(directory: &Path) -> Result<(VerifiableProof, [u8; 32], Value)> {
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
    let vk_hash: [u8; 32] = Sha256::digest(vk_json.as_bytes()).into();
    let hashes = json!({
        "vk.serde.json": hex::encode(vk_hash),
        "proof.serde.json": hash(proof_json.as_bytes()),
        "public_input_skeleton.json": hash(skeleton_json.as_bytes()),
        "app_statement.json": hash(app_json.as_bytes()),
    });
    Ok((proof, vk_hash, hashes))
}
