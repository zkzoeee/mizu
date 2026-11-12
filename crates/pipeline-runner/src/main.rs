//! Milestone 1 of the proof pipeline: core → compressed → PLONK, driven from
//! the pinned ELF and the manifest's inputs.
//!
//! ## What this is, and what it is not
//!
//! SP1's local node proves to a mode; it does **not** take an existing core
//! proof and compress it. `prove_with_mode` re-executes and re-proves from the
//! ELF and stdin, so the 132 MB core proof already on disk is not an input
//! here — the pipeline is driven from the program, not from the proof.
//!
//! Two switches make this runnable before any recursion program is compiled:
//!
//!   `without_vk_verification()`  builder.rs:294, behind `experimental`
//!   dummy/absent vk map          shapes.rs:485
//!
//! SP1's own comment on the first: "Turn off vk verification for recursion
//! proofs." Anything produced this way is a **development artifact**: with vk
//! verification off, nothing binds the recursion program that made the proof,
//! so a prover could substitute a different one. Label its outputs as such
//! wherever they are quoted — proof sizes and verification gas are meaningful,
//! soundness is not.
//!
//! Construction follows SP1's own `test_e2e_node_trap`
//! (`worker/node/full/mod.rs:396`) rather than an invented arrangement.
use std::{fs, path::PathBuf, time::Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use sp1_core_executor::SP1Context;
use sp1_core_machine::{io::SP1Stdin, riscv::RiscvAir};
use sp1_primitives::SP1Field;
use sp1_prover::worker::{cpu_worker_builder_with_machine, SP1LocalNodeBuilder};
use sp1_prover_types::network_base_types::ProofMode;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    /// Shards only. What the GPU harness already produces.
    Core,
    /// Recursion tree over the shards. The size collapse.
    Compressed,
    /// Compressed, shrunk, then wrapped for an EVM verifier.
    Plonk,
}

impl Mode {
    fn to_proof_mode(self) -> ProofMode {
        match self {
            Mode::Core => ProofMode::Core,
            Mode::Compressed => ProofMode::Compressed,
            Mode::Plonk => ProofMode::Plonk,
        }
    }
}

#[derive(Parser)]
struct Args {
    /// The pinned guest ELF.
    #[arg(long)]
    elf: PathBuf,
    /// Directory holding `proof-input.bin` and `receipt-input.bin`, as the
    /// bundle's `gpu/input/` does.
    #[arg(long)]
    input_dir: PathBuf,
    #[arg(long, value_enum, default_value = "compressed")]
    mode: Mode,
    #[arg(long)]
    output: PathBuf,
    /// Where to write the proof bytes. Omit to measure without keeping them.
    #[arg(long)]
    proof_out: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let elf = fs::read(&args.elf).context("read guest ELF")?;
    let proof_input =
        fs::read(args.input_dir.join("proof-input.bin")).context("read proof-input.bin")?;
    let receipt_input =
        fs::read(args.input_dir.join("receipt-input.bin")).context("read receipt-input.bin")?;

    // The verifier guest reads two raw buffers, in this order. Same as the
    // trace accounting does (`tools/pasta-proof/src/trace_accounting.rs`).
    let mut stdin = SP1Stdin::new();
    stdin.write_slice(&proof_input);
    stdin.write_slice(&receipt_input);

    let machine = RiscvAir::<SP1Field>::machine();
    let clusters = machine.shape().chip_clusters.len();

    let started = Instant::now();
    let node = SP1LocalNodeBuilder::from_worker_client_builder(
        cpu_worker_builder_with_machine(machine).without_vk_verification(),
    )
    .build()
    .await
    .context("build the local node")?;
    let node_built = started.elapsed();

    // A fixed nonce so two runs of the same inputs are comparable.
    let context = SP1Context { proof_nonce: [0, 0, 0, 0], ..Default::default() };

    let t = Instant::now();
    let (_public_values, _digest, report) = node
        .execute(&elf, stdin.clone(), context.clone())
        .await
        .context("execute the guest")?;
    let execute_seconds = t.elapsed().as_secs_f64();
    let cycles = report.total_instruction_count();

    let t = Instant::now();
    let vk = node.setup(&elf).await.context("setup")?;
    let setup_seconds = t.elapsed().as_secs_f64();

    let t = Instant::now();
    let proof = node
        .prove_with_mode(&elf, stdin, context, args.mode.to_proof_mode())
        .await
        .context("prove")?;
    let prove_seconds = t.elapsed().as_secs_f64();

    // Verify with the same node, so a proof that does not check fails here
    // rather than being reported as a size.
    let t = Instant::now();
    let verified = {
        let node = node.clone();
        let proof_for_verify = proof.proof.clone();
        tokio::task::spawn_blocking(move || node.verify(&vk, &proof_for_verify).is_ok())
            .await
            .context("join the verify task")?
    };
    let verify_seconds = t.elapsed().as_secs_f64();

    let proof_bytes = bincode::serialize(&proof.proof).context("serialize the proof")?;
    if let Some(path) = &args.proof_out {
        fs::write(path, &proof_bytes).context("write the proof")?;
    }

    let report_json = serde_json::json!({
        "mode": "pipeline-local",
        "proof_mode": format!("{:?}", args.mode),
        "soundness": "DEVELOPMENT ONLY: vk verification is off, so nothing binds the recursion program that produced this proof",
        "elf_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&elf)),
        "chip_clusters": clusters,
        "cycles": cycles,
        "verified": verified,
        "proof_bytes": proof_bytes.len(),
        "seconds": {
            "node_build": node_built.as_secs_f64(),
            "execute": execute_seconds,
            "setup": setup_seconds,
            "prove": prove_seconds,
            "verify": verify_seconds,
        },
    });
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.output, format!("{}\n", serde_json::to_string_pretty(&report_json)?))?;

    println!(
        "{:?}: {} cycles, {} proof bytes, verified {}, prove {:.1}s",
        args.mode, cycles, proof_bytes.len(), verified, prove_seconds
    );
    if !verified {
        anyhow::bail!("the proof did not verify; see {}", args.output.display());
    }
    Ok(())
}
