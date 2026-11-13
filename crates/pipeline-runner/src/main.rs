//! The CPU proof pipeline: core → compressed → Groth16, driven from the pinned
//! ELF and the manifest's inputs.
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
use std::{collections::BTreeSet, fs, path::PathBuf, time::Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use either::Either;
use sp1_core_executor::{RetainedEventsPreset, SP1Context, SP1CoreOpts};
use sp1_core_machine::{io::SP1Stdin, riscv::RiscvAir};
use sp1_primitives::SP1Field;
use sp1_prover::{
    shapes::{create_all_input_shapes, SP1RecursionProgramShape},
    worker::{cpu_worker_builder_with_machine, SP1LocalNodeBuilder},
    CpuSP1ProverComponents, SP1ProverComponents,
};
use sp1_prover_types::network_base_types::ProofMode;

/// `prover/src/shapes.rs:71`.
const DEFAULT_ARITY: usize = 4;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    /// Shards only. What the GPU harness already produces.
    Core,
    /// Compressed, shrunk, then wrapped for an EVM verifier. The only wrap this
    /// project targets. Needs the `native-gnark` feature, because the released
    /// artifacts supply the *keys* and gnark still does the proving; without it
    /// the FFI falls back to a Docker backend.
    ///
    /// Pass `--vk-map` and leave `SP1_CIRCUIT_MODE` unset. That turns vk
    /// verification on, so SP1's own wrap-vk check runs at wrap-prover init, and
    /// installs Succinct's released Groth16 artifacts -- no ceremony and no local
    /// setup, because this fork's wrap vk equals upstream's. Proved end to end on
    /// 2026-11-12; see docs/groth16-ceremony-spike.md and
    /// benchmarks/pipeline/groth16-20261112/.
    ///
    /// Do NOT reach for `SP1_CIRCUIT_MODE=dev`. It builds the keys locally with
    /// gnark's own *unsafe* setup, needs 107.4 GiB, and produces artifacts whose
    /// soundness is worthless. It exists only as the path this project took before
    /// the wrap vk was shown to match.
    Groth16,
    /// Build a partial recursion VK map for exactly this workload, the way
    /// SP1's own `test_build_vk_map` does (`prover/src/shapes.rs:1037`): prove
    /// core, collect the recursion shape of every shard, add the last 12 shapes
    /// (compose, deferred, shrink), and build the vks for those alone. Writes
    /// the map to `--vk-map`; `from_map` pads the rest of the tree.
    ///
    /// This is what makes `vk_verification` usable without building all
    /// 249,750 programs. The cost is generality: a shard whose shape is not in
    /// the map cannot be proved until the map is extended, and extending it
    /// changes the root, which the verifier contract pins as `VK_ROOT`.
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
    /// The pinned guest ELF.
    #[arg(long)]
    elf: PathBuf,
    /// Directory holding `proof-input.bin` and `receipt-input.bin`, as the
    /// bundle's `gpu/input/` does.
    #[arg(long)]
    input_dir: PathBuf,
    #[arg(long, value_enum, default_value = "groth16")]
    mode: Mode,
    #[arg(long)]
    output: PathBuf,
    /// Where to write the proof bytes. Omit to measure without keeping them.
    #[arg(long)]
    proof_out: Option<PathBuf>,
    /// Execute the guest once up front purely to report its cycle count.
    /// Off by default: `prove_with_mode` re-executes from the ELF anyway, so
    /// this is the same work twice -- 26.0 s of run C's 350.9 s on 2026-11-13.
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    count_cycles: bool,
    /// Retain the Pasta and UInt256 events instead of deferring them, as the
    /// `pasta-proof` harness's retained arm does and as `vk-map-local` assumes.
    /// Changes how shards are cut, so the VK map must have been built with the
    /// same setting or its membership check will fail.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    retain_pasta: bool,
    /// The recursion VK map. `--mode vk-map` writes it; every other mode reads
    /// it and turns vk verification ON. Omit it for a development run with
    /// verification off.
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

    // A fixed nonce so two runs of the same inputs are comparable.
    let context = SP1Context { proof_nonce: [0, 0, 0, 0], ..Default::default() };

    if matches!(args.mode, Mode::VkMap) {
        let path = args.vk_map.clone().context("--mode vk-map needs --vk-map <path>")?;
        let core_verifier = CpuSP1ProverComponents::core_verifier(machine.clone());
        let mut vk_builder = cpu_worker_builder_with_machine(machine);
        if args.retain_pasta {
            let mut o = vk_builder.core_opts().clone();
            apply_retention(&mut o);
            vk_builder = vk_builder.with_core_opts(o);
        }
        let node = SP1LocalNodeBuilder::from_worker_client_builder(vk_builder)
        .build()
        .await
        .context("build the local node")?;

        let t = Instant::now();
        let proof = node
            .prove_with_mode(&elf, stdin, context, ProofMode::Core)
            .await
            .context("prove core")?;
        let core_seconds = t.elapsed().as_secs_f64();
        let shards = proof.proof.try_as_core_ref().context("expected a core proof")?;

        let shapes = create_all_input_shapes(
            core_verifier.shard_verifier().machine().shape(),
            DEFAULT_ARITY,
        );
        let mut indices = BTreeSet::new();
        for shard in shards {
            let shape = SP1RecursionProgramShape::Normalize(core_verifier.shape_from_proof(shard));
            let index = shapes
                .iter()
                .position(|s| s == &shape)
                .context("a shard's shape is not in create_all_input_shapes")?;
            indices.insert(index);
        }
        let distinct_shard_shapes = indices.len();
        indices.extend(shapes.len() - 12..shapes.len());

        let t = Instant::now();
        let result = node
            .build_vks(Some(Either::Left(indices.iter().copied().collect())), 4)
            .await
            .context("build_vks")?;
        let build_seconds = t.elapsed().as_secs_f64();
        anyhow::ensure!(
            result.panic_indices.is_empty(),
            "build_vks panicked on shape indices {:?}",
            result.panic_indices
        );

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, bincode::serialize(&result.vk_map).context("serialize the vk map")?)
            .context("write the vk map")?;

        let report_json = serde_json::json!({
            "mode": "pipeline-local",
            "proof_mode": "VkMap",
            "elf_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&elf)),
            "chip_clusters": clusters,
            "shards": shards.len(),
            "distinct_shard_shapes": distinct_shard_shapes,
            "shape_indices": indices,
            "shape_space": shapes.len(),
            "vk_map_keys": result.vk_map.len(),
            "vk_map_path": path.display().to_string(),
            "seconds": { "core": core_seconds, "build_vks": build_seconds },
        });
        if let Some(parent) = args.output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&args.output, format!("{}\n", serde_json::to_string_pretty(&report_json)?))?;
        println!(
            "vk map: {} shards, {} distinct shard shapes, {} keys built in {:.0}s -> {}",
            shards.len(),
            distinct_shard_shapes,
            result.vk_map.len(),
            build_seconds,
            path.display()
        );
        return Ok(());
    }

    let started = Instant::now();
    let mut builder = cpu_worker_builder_with_machine(machine);
    if args.retain_pasta {
        let mut o = builder.core_opts().clone();
        apply_retention(&mut o);
        builder = builder.with_core_opts(o);
    }
    let builder = match &args.vk_map {
        Some(path) => builder.with_vk_map_path(path.display().to_string()),
        None => builder.without_vk_verification(),
    };
    let node = SP1LocalNodeBuilder::from_worker_client_builder(builder)
        .build()
        .await
        .context("build the local node")?;
    let node_built = started.elapsed();

    // Opt-in: prove_with_mode re-executes from the ELF regardless, so this is
    // the same work twice.
    let (cycles, execute_seconds) = if args.count_cycles {
        let t = Instant::now();
        let (_public_values, _digest, report) = node
            .execute(&elf, stdin.clone(), context.clone())
            .await
            .context("execute the guest")?;
        (report.total_instruction_count(), t.elapsed().as_secs_f64())
    } else {
        (0u64, 0.0)
    };

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

    // The Groth16 proof proper, when that is what was made.
    let groth16 = proof.proof.try_as_groth_16_ref().map(|g| {
        serde_json::json!({
            "public_inputs": g.public_inputs,
            "vk_root": g.public_inputs[3],
            "raw_proof_bytes": g.raw_proof.len() / 2,
            "encoded_proof_bytes": g.encoded_proof.len() / 2,
            "groth16_vkey_hash": hex::encode(g.groth16_vkey_hash),
        })
    });

    let report_json = serde_json::json!({
        "mode": "pipeline-local",
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
        "chip_clusters": clusters,
        "cycles": if cycles == 0 { serde_json::Value::Null } else { cycles.into() },
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
