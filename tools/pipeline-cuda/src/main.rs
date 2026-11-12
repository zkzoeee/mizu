//! The CUDA proof pipeline: core → compressed → PLONK, on one GPU.
//!
//! Why this exists separately from `crates/pipeline-runner`: that one is the
//! CPU driver and lives in the root workspace, which is `resolver = "2"`. A
//! CUDA build cannot, for the reason this crate's `Cargo.toml` records. The two
//! drivers do the same job on different hardware and share no code, because
//! sharing it would mean one workspace.
//!
//! Why CUDA at all: CPU core proving of this guest was measured on 2026-11-12
//! needing more than 64 GB — two runs OOM-killed at a 59.6 GB peak, with a
//! 46–49 GB plateau that a four-thread cap did not lower. The GPU prover did
//! the same arm in 100.8 s with 18,991 MiB of VRAM, eleven times. See
//! `docs/proof-pipeline-plan.md`.
//!
//! ## Development only, and the reason travels with the output
//!
//! `without_vk_verification()` (`sp1-prover` `builder.rs:294`, behind
//! `experimental`) switches off the recursion VK membership check, so nothing
//! binds the recursion program that produced a proof and a prover could
//! substitute a different one. SP1's own comment: "Turn off vk verification for
//! recursion proofs." Proof sizes and verification gas from this are
//! meaningful; soundness is not. The report JSON says so in a field, so the
//! caveat cannot be separated from the numbers.
//!
//! Building the real VK map — 54 clusters × 6,001 shapes = 324,060 recursion
//! programs — is Phase A task 3 and is what makes this sound.
use std::{fs, path::PathBuf, time::Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use sp1_core_executor::SP1Context;
use sp1_core_machine::{io::SP1Stdin, riscv::RiscvAir};
use sp1_gpu_prover::cuda_worker_builder_with_machine;
use sp1_primitives::SP1Field;
use sp1_prover::worker::SP1LocalNodeBuilder;
use sp1_prover_types::network_base_types::ProofMode;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    Core,
    Compressed,
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
    #[arg(long)]
    elf: PathBuf,
    /// Directory holding `proof-input.bin` and `receipt-input.bin`.
    #[arg(long)]
    input_dir: PathBuf,
    #[arg(long, value_enum, default_value = "compressed")]
    mode: Mode,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    proof_out: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();

    let elf = fs::read(&args.elf).context("read guest ELF")?;
    let proof_input =
        fs::read(args.input_dir.join("proof-input.bin")).context("read proof-input.bin")?;
    let receipt_input =
        fs::read(args.input_dir.join("receipt-input.bin")).context("read receipt-input.bin")?;

    // The verifier guest reads two raw buffers, in this order.
    let mut stdin = SP1Stdin::new();
    stdin.write_slice(&proof_input);
    stdin.write_slice(&receipt_input);

    let machine = RiscvAir::<SP1Field>::machine();
    let mode = args.mode;
    let elf_for_task = elf.clone();

    // Everything that touches the device has to happen inside the task scope:
    // a `TaskScope` is a weak handle into a pooled CUDA task and is valid only
    // for that task's lifetime. Same constraint `tools/pasta-proof/src/gpu.rs`
    // documents at its own spawn site.
    let handle = sp1_gpu_cudart::spawn(move |scope| async move {
        let t = Instant::now();
        // `local_gpu_opts()` inside the builder panics under 24 GB of VRAM and
        // sets shard_size to 1 << 24, so a GPU proof has fewer, larger shards
        // than the CPU default.
        let node = SP1LocalNodeBuilder::from_worker_client_builder(
            cuda_worker_builder_with_machine(scope, machine).await.without_vk_verification(),
        )
        .build()
        .await
        .expect("build the cuda node");
        let node_built = t.elapsed().as_secs_f64();

        let context = SP1Context { proof_nonce: [0, 0, 0, 0], ..Default::default() };

        let t = Instant::now();
        let (_pv, _digest, report) = node
            .execute(&elf_for_task, stdin.clone(), context.clone())
            .await
            .expect("execute the guest");
        let execute_seconds = t.elapsed().as_secs_f64();
        let cycles = report.total_instruction_count();

        let t = Instant::now();
        let vk = node.setup(&elf_for_task).await.expect("setup");
        let setup_seconds = t.elapsed().as_secs_f64();

        let t = Instant::now();
        let proof = node
            .prove_with_mode(&elf_for_task, stdin, context, mode.to_proof_mode())
            .await
            .expect("prove");
        let prove_seconds = t.elapsed().as_secs_f64();

        // Verify what we just made, so a proof size can never be reported for
        // a proof that does not check.
        let t = Instant::now();
        let verified = node.verify(&vk, &proof.proof).is_ok();
        let verify_seconds = t.elapsed().as_secs_f64();

        (cycles, node_built, execute_seconds, setup_seconds, prove_seconds, verify_seconds, verified, proof.proof)
    });

    // `SpawnHandle<T>` implements `IntoFuture` with `Output = Result<T, SpawnError>`;
    // it has no `join`. Same await as `tools/pasta-proof/src/gpu.rs:308`.
    let (cycles, node_built, execute_seconds, setup_seconds, prove_seconds, verify_seconds, verified, proof) =
        handle.await.map_err(|e| anyhow::anyhow!("the CUDA task failed: {e:?}"))?;

    let proof_bytes = bincode::serialize(&proof).context("serialize the proof")?;
    if let Some(path) = &args.proof_out {
        fs::write(path, &proof_bytes).context("write the proof")?;
    }

    let report_json = serde_json::json!({
        "mode": "pipeline-cuda",
        "proof_mode": format!("{:?}", args.mode),
        "soundness": "DEVELOPMENT ONLY: vk verification is off, so nothing binds the recursion program that produced this proof",
        "elf_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&elf)),
        "cycles": cycles,
        "verified": verified,
        "proof_bytes": proof_bytes.len(),
        "seconds": {
            "node_build": node_built,
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
