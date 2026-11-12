//! The CUDA proof pipeline: core → compressed → Groth16, on one GPU.
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
use std::{collections::BTreeSet, fs, path::PathBuf, time::Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use either::Either;
use sp1_core_executor::{RetainedEventsPreset, SP1Context, SP1CoreOpts};
use sp1_core_machine::{io::SP1Stdin, riscv::RiscvAir};
use sp1_gpu_prover::cuda_worker_builder_with_machine;
use sp1_primitives::SP1Field;
use sp1_prover::{
    shapes::{create_all_input_shapes, SP1RecursionProgramShape},
    worker::SP1LocalNodeBuilder,
    CpuSP1ProverComponents, SP1ProverComponents,
};
use sp1_prover_types::network_base_types::ProofMode;

/// `prover/src/shapes.rs:71`.
const DEFAULT_ARITY: usize = 4;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    Core,
    /// The only wrap this project targets. Needs the `native-gnark` feature,
    /// because the released artifacts supply the *keys* and gnark still does the
    /// proving; without it the FFI falls back to a Docker backend.
    ///
    /// Pass `--vk-map` and leave `SP1_CIRCUIT_MODE` unset: vk verification on,
    /// SP1's own wrap-vk check runs, and Succinct's released Groth16 artifacts
    /// install -- no ceremony, no local setup. Proved on an A40 2026-11-12, 497 s
    /// cold / about 6 min warm, 324-byte proof; see
    /// benchmarks/pipeline/groth16-20261112/.
    ///
    /// Do NOT use `SP1_CIRCUIT_MODE=dev`: gnark's own unsafe local setup, 107.4
    /// GiB, and soundness worth nothing. It is only the path taken before the
    /// wrap vk was shown to match upstream's.
    Groth16,
    /// Build a partial recursion VK map for exactly this workload, as SP1's own
    /// `test_build_vk_map` does (`prover/src/shapes.rs:1037`): prove core,
    /// collect the recursion shape of every shard, add the last 12 shapes
    /// (compose, deferred, shrink), and build the vks for those alone. Writes
    /// the map to `--vk-map`; `from_map` pads the rest of the tree.
    ///
    /// Run it on the same pod and build as the Groth16 run that will use the
    /// map: the shard shapes must be the ones that run produces.
    VkMap,
}

impl Mode {
    fn to_proof_mode(self) -> ProofMode {
        match self {
            Mode::Core => ProofMode::Core,
            Mode::Groth16 => ProofMode::Groth16,
            // Never reached: main() handles VkMap before this is called.
            Mode::VkMap => ProofMode::Core,
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
    #[arg(long, value_enum, default_value = "groth16")]
    mode: Mode,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    proof_out: Option<PathBuf>,
    /// Retain the Pasta and UInt256 events instead of deferring them, as the
    /// `pasta-proof` harness's retained arm does and as `vk-map-local` assumes.
    /// Changes how shards are cut, so the VK map must have been built with the
    /// same setting or its membership check will fail.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    retain_pasta: bool,
    /// The recursion VK map. `--mode vk-map` writes it; every other mode reads
    /// it and turns vk verification ON, which is what makes the result real:
    /// SP1's own wrap-vk check then runs, and with `SP1_CIRCUIT_MODE` unset the
    /// released Groth16 artifacts are installed. Omit for a development run.
    #[arg(long)]
    vk_map: Option<PathBuf>,
}

/// The Pasta/UInt256 retention this project proves with, applied on top of
/// whatever the prover's own options already retain. Must match the setting the
/// VK map was built with (`vk-map-local --retain-pasta`), because retention
/// changes how shards are cut and therefore their shapes.
fn apply_retention(opts: &mut SP1CoreOpts) {
    opts.retained_events_presets.insert(RetainedEventsPreset::Uint256MulMod);
    opts.retained_events_presets.insert(RetainedEventsPreset::VestaAdd);
    opts.retained_events_presets.insert(RetainedEventsPreset::PallasAdd);
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

    if matches!(mode, Mode::VkMap) {
        let path = args.vk_map.clone().context("--mode vk-map needs --vk-map <path>")?;
        let clusters = machine.shape().chip_clusters.len();
        let elf_for_map = elf.clone();
        let retain = args.retain_pasta;
        let handle = sp1_gpu_cudart::spawn(move |scope| async move {
            let core_verifier = CpuSP1ProverComponents::core_verifier(machine.clone());
            let mut vk_builder = cuda_worker_builder_with_machine(scope, machine).await;
            if retain {
                let mut o = vk_builder.core_opts().clone();
                apply_retention(&mut o);
                vk_builder = vk_builder.with_core_opts(o);
            }
            let node = SP1LocalNodeBuilder::from_worker_client_builder(vk_builder)
            .build()
            .await
            .expect("build the cuda node");
            let context = SP1Context { proof_nonce: [0, 0, 0, 0], ..Default::default() };

            let t = Instant::now();
            let proof = node
                .prove_with_mode(&elf_for_map, stdin, context, ProofMode::Core)
                .await
                .expect("prove core");
            let core_seconds = t.elapsed().as_secs_f64();
            let shards = proof.proof.try_as_core_ref().expect("expected a core proof");

            let shapes = create_all_input_shapes(
                core_verifier.shard_verifier().machine().shape(),
                DEFAULT_ARITY,
            );
            let mut indices = BTreeSet::new();
            for shard in shards {
                let shape =
                    SP1RecursionProgramShape::Normalize(core_verifier.shape_from_proof(shard));
                let index = shapes
                    .iter()
                    .position(|s| s == &shape)
                    .expect("a shard's shape is not in create_all_input_shapes");
                indices.insert(index);
            }
            let distinct = indices.len();
            indices.extend(shapes.len() - 12..shapes.len());

            let t = Instant::now();
            let result = node
                .build_vks(Some(Either::Left(indices.iter().copied().collect())), 4)
                .await
                .expect("build_vks");
            let build_seconds = t.elapsed().as_secs_f64();
            assert!(
                result.panic_indices.is_empty(),
                "build_vks panicked on shape indices {:?}",
                result.panic_indices
            );
            let map_bytes = bincode::serialize(&result.vk_map).expect("serialize the vk map");
            (
                shards.len(),
                distinct,
                indices.into_iter().collect::<Vec<_>>(),
                shapes.len(),
                result.vk_map.len(),
                map_bytes,
                core_seconds,
                build_seconds,
            )
        });
        let (shards, distinct, indices, shape_space, keys, map_bytes, core_seconds, build_seconds) =
            handle.await.map_err(|e| anyhow::anyhow!("the CUDA task failed: {e:?}"))?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &map_bytes).context("write the vk map")?;
        let report_json = serde_json::json!({
            "mode": "pipeline-cuda",
            "proof_mode": "VkMap",
            "elf_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&elf)),
            "chip_clusters": clusters,
            "shards": shards,
            "distinct_shard_shapes": distinct,
            "shape_indices": indices,
            "shape_space": shape_space,
            "vk_map_keys": keys,
            "vk_map_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&map_bytes)),
            "vk_map_path": path.display().to_string(),
            "seconds": { "core": core_seconds, "build_vks": build_seconds },
        });
        if let Some(parent) = args.output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&args.output, format!("{}\n", serde_json::to_string_pretty(&report_json)?))?;
        println!(
            "vk map: {shards} shards, {distinct} distinct shard shapes, {keys} keys built in {build_seconds:.0}s -> {}",
            path.display()
        );
        return Ok(());
    }

    let vk_map_for_task = args.vk_map.as_ref().map(|p| p.display().to_string());
    let retain = args.retain_pasta;

    // Everything that touches the device has to happen inside the task scope:
    // a `TaskScope` is a weak handle into a pooled CUDA task and is valid only
    // for that task's lifetime. Same constraint `tools/pasta-proof/src/gpu.rs`
    // documents at its own spawn site.
    let handle = sp1_gpu_cudart::spawn(move |scope| async move {
        let t = Instant::now();
        // `local_gpu_opts()` inside the builder panics under 24 GB of VRAM and
        // sets shard_size to 1 << 24, so a GPU proof has fewer, larger shards
        // than the CPU default.
        let mut builder = cuda_worker_builder_with_machine(scope, machine).await;
        if retain {
            let mut o = builder.core_opts().clone();
            apply_retention(&mut o);
            builder = builder.with_core_opts(o);
        }
        let builder = match vk_map_for_task {
            Some(path) => builder.with_vk_map_path(path),
            None => builder.without_vk_verification(),
        };
        let node = SP1LocalNodeBuilder::from_worker_client_builder(builder)
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

    // The Groth16 proof proper, when that is what was made.
    let groth16 = proof.try_as_groth_16_ref().map(|g| {
        serde_json::json!({
            "public_inputs": g.public_inputs,
            "vk_root": g.public_inputs[3],
            "raw_proof_bytes": g.raw_proof.len() / 2,
            "encoded_proof_bytes": g.encoded_proof.len() / 2,
            "groth16_vkey_hash": hex::encode(g.groth16_vkey_hash),
        })
    });

    let report_json = serde_json::json!({
        "mode": "pipeline-cuda",
        "proof_mode": format!("{:?}", args.mode),
        "vk_verification": args.vk_map.is_some(),
        "retain_pasta": args.retain_pasta,
        "groth16": groth16,
        "soundness": if args.vk_map.is_some() {
            "vk verification ON against the supplied partial vk map; the recursion programs are bound by its root"
        } else {
            "DEVELOPMENT ONLY: vk verification is off, so nothing binds the recursion program that produced this proof"
        },
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
