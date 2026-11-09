//! CPU-only small operation benchmarks. No SDK, prover or GPU entry point.
use anyhow::{anyhow, bail, Context, Result};
use ark_ff::PrimeField;
use clap::Parser;
use mina_curves::pasta::{Fp, Fq};
use mina_poseidon::{
    constants::PlonkSpongeConstantsKimchi,
    pasta::{fp_kimchi, fq_kimchi},
    permutation::poseidon_block_cipher,
    poseidon::ArithmeticSpongeParams,
};
use sha2::{Digest, Sha256};
use sp1_core_executor::Program;
use sp1_core_executor_runner::MinimalExecutorRunner;
use std::{fs, path::PathBuf, sync::Arc, time::Instant};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    elf: PathBuf,
    #[arg(long, default_value_t=16, value_parser=clap::value_parser!(u32).range(1..=64))]
    iterations: u32,
    #[arg(long)]
    output: PathBuf,
}
fn append_reference<F: PrimeField>(
    out: &mut Vec<u8>,
    params: &ArithmeticSpongeParams<F, 55>,
    iterations: u32,
) {
    let mut state = [F::ZERO, F::ONE, -F::ONE];
    for _ in 0..iterations {
        poseidon_block_cipher::<F, PlonkSpongeConstantsKimchi, 55>(params, &mut state);
    }
    for value in state {
        for limb in value.into_bigint().as_ref() {
            out.extend_from_slice(&limb.to_le_bytes());
        }
    }
}
fn reference(iterations: u32) -> Vec<u8> {
    let mut out = iterations.to_le_bytes().to_vec();
    // Native permutation dispatch remains the ordinary Mina implementation.
    append_reference::<Fp>(&mut out, fp_kimchi::static_params(), iterations);
    append_reference::<Fq>(&mut out, fq_kimchi::static_params(), iterations);
    out
}
fn main() -> Result<()> {
    let args = Args::parse();
    let expected = reference(args.iterations);
    let input = bincode::serialize(&args.iterations)?;
    let elf = fs::read(&args.elf).context("read operation guest")?;
    let started = Instant::now();
    let program = Arc::new(
        Program::from(elf.as_slice()).map_err(|e| anyhow!("disassemble operation guest: {e}"))?,
    );
    let mut runner = MinimalExecutorRunner::new(program, false, None, 2 * 1024 * 1024 * 1024, 0);
    runner.with_input(&input);
    let failure = loop {
        match runner.try_execute_chunk() {
            Ok(Some(_)) => {}
            Ok(None) => break None,
            Err(error) => break Some(format!("execute: {error}")),
        }
    };
    let cycles = runner.global_clk();
    let exit = runner.exit_code();
    let stages = runner.take_cycle_tracker_totals();
    let uint256 = runner.uint256_mul_count();
    let pasta = runner.pasta_syscall_counts();
    let actual = runner.into_public_values_stream();
    let accepted = failure.is_none() && exit == 0 && actual == expected && pasta == [0; 4];
    let report = serde_json::json!({"mode":"cpu-operation-benchmark","operation":"mina-kimchi-poseidon-fp-fq","iterations_per_field":args.iterations,"elf_sha256":hex::encode(Sha256::digest(&elf)),"input_sha256":hex::encode(Sha256::digest(&input)),"cycles":cycles,"stage_cycles":stages,"uint256_mul_count":uint256,"pasta_syscall_counts":pasta,"guest_exit_code":exit,"accepted":accepted,"failure":failure,"public_values_len":actual.len(),"public_values_sha256":hex::encode(Sha256::digest(&actual)),"expected_public_values_sha256":hex::encode(Sha256::digest(&expected)),"elapsed_seconds":started.elapsed().as_secs_f64()});
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &args.output,
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    println!("cycles: {cycles}; accepted: {accepted}");
    if !accepted {
        bail!("operation benchmark failed: {}", args.output.display());
    }
    Ok(())
}
