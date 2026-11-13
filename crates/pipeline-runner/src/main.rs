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
/// `prover/src/components.rs:16`.
const LOG_STACKING_HEIGHT: usize = 21;

/// The fixed programs `create_all_input_shapes` appends after the Normalize
/// shapes: one Compose per arity, then Deferred and Shrink
/// (`prover/src/shapes.rs:730-738`). Taking a wider tail than this pulls in
/// arbitrary Normalize shapes that are built, never used, and still change the
/// map's root -- this said 12 until 2026-11-13.
const FIXED_TAIL: usize = DEFAULT_ARITY + 2;

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
    /// core, collect the recursion shape of every shard, add the fixed tail
    /// (one compose per arity, deferred, shrink), and build the vks for those
    /// alone. Writes
    /// the map to `--vk-map`; `from_map` pads the rest of the tree.
    ///
    /// This is what makes `vk_verification` usable without building all
    /// 249,750 programs. The cost is generality: a shard whose shape is not in
    /// the map cannot be proved until the map is extended, and extending it
    /// changes the root, which the verifier contract pins as `VK_ROOT`.
    VkMap,
    /// Build the VK map for a whole preprocessed block, without proving
    /// anything. `--mode vk-map` covers only the shapes one input happened to
    /// produce; this covers every shape any input to a program in this block
    /// can produce, because the preprocessed area is fixed by the program while
    /// the cluster and the main multiple are what vary. One of these outlives
    /// the input, so the root it pins in the verifier contract stops moving.
    ///
    /// `--block` picks the preprocessed multiple (3 is our guest's; run
    /// `vk-map-local --shapes-only` against an ELF to find its own). `--sample`
    /// builds a spread of the block instead of all of it, which is how to
    /// measure the rate and the memory before committing to the full run --
    /// this build has no resume, and the controller holds every chunk's
    /// artifacts until the end (`worker/controller/vk_tree.rs:95`).
    ///
    /// Ignores `--elf` and `--input-dir`.
    VkMapBlock,
    /// Prove compressed, then shrink and wrap, and save the wrap proof.
    ///
    /// The wrap proof is the last STARK artifact and the only thing the Groth16
    /// circuit reads (`worker/prover/recursion.rs:732` builds its whole witness
    /// from `wrap_proof.vk` and `wrap_proof.proof`), so saving it lets a later
    /// Groth16 run skip execution, core proving and the reduce tree.
    ///
    /// Only valid for re-proving the SAME input: the wrap proof's public values
    /// carry the vk digest, the committed values digest and `vk_root`, so it is
    /// stale the moment the guest, the input, the retention config or the map
    /// changes.
    Wrap,
    /// Groth16-prove a wrap proof saved earlier by `--mode wrap`. No GPU, no
    /// execution, no core proving, no reduce tree -- the Groth16 circuit reads
    /// nothing but the wrap proof. This is the cheap half of the pipeline, and
    /// the only one that has to re-run when the gnark side changes.
    ///
    /// Ignores `--elf` and `--input-dir`.
    FromWrap,
    /// Execute the guest and report what it committed. No proving, no GPU,
    /// seconds.
    ///
    /// `verifyProof` needs the raw public values, because it recomputes
    /// `sha256(publicValues) & mask` and compares that against public input 1.
    /// Every other mode records the *hashed* inputs, which is not enough to
    /// call the contract -- so a proof could be made and then not be checkable
    /// on chain for want of 76 bytes. This dumps them.
    ///
    /// Public values are a function of the guest and its input alone, so this
    /// recovers them for a proof that was already made elsewhere.
    Execute,
}

impl Mode {
    fn to_proof_mode(self) -> ProofMode {
        match self {
            Mode::Core => ProofMode::Core,
            Mode::Groth16 => ProofMode::Groth16,
            // Wrap proves compressed, then shrink-wraps the result itself.
            Mode::Wrap => ProofMode::Compressed,
            // Never reached: main() handles these before this is called.
            Mode::VkMap | Mode::VkMapBlock | Mode::FromWrap | Mode::Execute => ProofMode::Core,
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
    /// `--mode wrap`: where to write the wrap proof.
    #[arg(long)]
    wrap_out: Option<PathBuf>,
    /// `--mode from-wrap`: the wrap proof to Groth16-prove.
    #[arg(long)]
    wrap_in: Option<PathBuf>,
    /// `--mode execute`: where to write the raw committed public values.
    #[arg(long)]
    public_values_out: Option<PathBuf>,
    /// `--mode vk-map-block`: the preprocessed multiple to cover.
    #[arg(long, default_value = "3")]
    block: usize,
    /// `--mode vk-map-block`: build only this many indices, spread evenly over
    /// the block and always including both ends. For rate and memory probing.
    #[arg(long)]
    sample: Option<usize>,
    /// `--mode vk-map-block`: build only slice `i` of `--slice-count`, so the
    /// block fits a machine. `build_vks` holds every built vk in the local
    /// in-memory artifact client until it merges, so RSS grows with the program
    /// count: a 50 GB container OOMs at about 126 programs (measured
    /// 2026-11-13). Slices are contiguous and disjoint, and the fixed tail is
    /// built with slice 0 only. Merge the results with tools/merge-vkmap.py.
    #[arg(long)]
    slice_index: Option<usize>,
    /// How many slices the block is cut into.
    #[arg(long, default_value = "1")]
    slice_count: usize,
    /// `build_vks` chunk size. The controller submits every chunk up front, so
    /// this trades task count against per-task memory.
    #[arg(long, default_value = "4")]
    chunk: usize,
    /// Keep going when `build_vks` panics on a shape, recording the indices in
    /// the report instead of failing. `build_vk_map` collects panicking shapes
    /// rather than failing (`worker/prover/vk_worker.rs:39`), so the default is
    /// to refuse: a map missing a shape still builds, and the shard that lands
    /// on it fails much later with `vk not allowed`.
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    allow_panics: bool,
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

    // No ELF, no input, no node: the Groth16 circuit's whole witness comes out
    // of the wrap proof.
    if matches!(args.mode, Mode::FromWrap) {
        let path = args.wrap_in.clone().context("--mode from-wrap needs --wrap-in <path>")?;
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let wrap: sp1_hypercube::SP1WrapProof<
            sp1_primitives::SP1OuterGlobalContext,
            sp1_hypercube::SP1PcsProofOuter,
        > = bincode::deserialize(&bytes).context("deserialize the wrap proof")?;
        println!("wrap proof: {} bytes from {}", bytes.len(), path.display());
        let t = Instant::now();
        let proof = sp1_prover::build::groth16_from_wrap(wrap).await?;
        let seconds = t.elapsed().as_secs_f64();
        let report = serde_json::json!({
            "mode": "pipeline-local",
            "proof_mode": "FromWrap",
            "wrap_in": path.display().to_string(),
            "wrap_proof_bytes": bytes.len(),
            "groth16": {
                "public_inputs": proof.public_inputs,
                "raw_proof_bytes": proof.raw_proof.len() / 2,
                "encoded_proof_bytes": proof.encoded_proof.len() / 2,
                "groth16_vkey_hash": hex::encode(proof.groth16_vkey_hash),
            },
            "seconds": { "groth16": seconds },
            "note": "verified inside groth16_from_wrap before it returned",
        });
        if let Some(parent) = args.output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&args.output, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
        if let Some(path) = &args.proof_out {
            fs::write(path, bincode::serialize(&proof).context("serialize the proof")?)?;
        }
        println!("groth16 in {seconds:.1}s, {} raw bytes", proof.raw_proof.len() / 2);
        return Ok(());
    }

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

    if matches!(args.mode, Mode::Execute) {
        let node = SP1LocalNodeBuilder::from_worker_client_builder(
            cpu_worker_builder_with_machine(machine),
        )
        .build()
        .await
        .context("build the local node")?;
        let context = SP1Context { proof_nonce: [0, 0, 0, 0], ..Default::default() };
        let t = Instant::now();
        let (public_values, _digest, report) = node
            .execute(&elf, stdin, context)
            .await
            .context("execute the guest")?;
        let seconds = t.elapsed().as_secs_f64();
        let bytes = public_values.as_slice().to_vec();
        // What the contract does with them, so a mismatch shows up here and not
        // in a failing forge test: sha256 masked to 253 bits is public input 1.
        let digest = <sha2::Sha256 as sha2::Digest>::digest(&bytes);
        let mut masked = [0u8; 32];
        masked.copy_from_slice(&digest);
        masked[0] &= 0x1f;
        if let Some(path) = &args.public_values_out {
            fs::write(path, &bytes).context("write the public values")?;
        }
        let report_json = serde_json::json!({
            "mode": "pipeline-local",
            "proof_mode": "Execute",
            "elf_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&elf)),
            "cycles": report.total_instruction_count(),
            "public_values_len": bytes.len(),
            "public_values": hex::encode(&bytes),
            "public_values_sha256": hex::encode(digest),
            "hash_public_values": format!("0x{}", hex::encode(masked)),
            "note": "hash_public_values is sha256(publicValues) & ((1<<253)-1), which SP1VerifierGroth16 compares against public input 1",
            "seconds": { "execute": seconds },
        });
        if let Some(parent) = args.output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&args.output, format!("{}\n", serde_json::to_string_pretty(&report_json)?))?;
        println!(
            "{} cycles, {} public-value bytes: 0x{}",
            report.total_instruction_count(),
            bytes.len(),
            hex::encode(&bytes)
        );
        return Ok(());
    }


    // A fixed nonce so two runs of the same inputs are comparable.
    let context = SP1Context { proof_nonce: [0, 0, 0, 0], ..Default::default() };

    if matches!(args.mode, Mode::VkMapBlock) {
        let path = args.vk_map.clone().context("--mode vk-map-block needs --vk-map <path>")?;
        let shapes = create_all_input_shapes(machine.shape(), DEFAULT_ARITY);

        // Select by reading the enumeration rather than by index arithmetic:
        // the block is whichever Normalize shapes carry this preprocessed
        // multiple, and `shape-space-audit` is what checks that they form one
        // contiguous run.
        let mut block: Vec<usize> = Vec::new();
        for (i, shape) in shapes.iter().enumerate() {
            if let SP1RecursionProgramShape::Normalize(core) = shape {
                if core.preprocessed_area >> LOG_STACKING_HEIGHT == args.block {
                    block.push(i);
                }
            }
        }
        anyhow::ensure!(!block.is_empty(), "no shapes at preprocessed multiple {}", args.block);
        let block_len = block.len();

        anyhow::ensure!(args.slice_count >= 1, "--slice-count must be at least 1");
        let selected: Vec<usize> = match (args.slice_index, args.sample) {
            // One contiguous slice of the block. Disjoint across indices, and
            // together they cover it exactly.
            (Some(i), _) => {
                anyhow::ensure!(
                    i < args.slice_count,
                    "--slice-index {i} is out of range for --slice-count {}",
                    args.slice_count
                );
                let lo = i * block_len / args.slice_count;
                let hi = (i + 1) * block_len / args.slice_count;
                block[lo..hi].to_vec()
            }
            // Evenly spread, both ends included: the extremes are the shapes
            // most likely to panic and the least likely to have been built.
            (None, Some(n)) if n < block_len => {
                let mut v: BTreeSet<usize> = BTreeSet::new();
                for k in 0..n.max(2) {
                    let pos = k * (block_len - 1) / (n.max(2) - 1);
                    v.insert(block[pos]);
                }
                v.into_iter().collect()
            }
            _ => block.clone(),
        };
        let mut indices: BTreeSet<usize> = selected.iter().copied().collect();
        // The fixed tail rides with slice 0, so the slices stay disjoint.
        if args.slice_index.unwrap_or(0) == 0 {
            indices.extend(shapes.len() - FIXED_TAIL..shapes.len());
        }

        println!(
            "block {}: {block_len} shapes (indices {}..{}), building {} + {FIXED_TAIL} fixed",
            args.block,
            block[0],
            block[block_len - 1] + 1,
            selected.len()
        );

        let node = SP1LocalNodeBuilder::from_worker_client_builder(
            cpu_worker_builder_with_machine(machine),
        )
        .build()
        .await
        .context("build the local node")?;
        let t = Instant::now();
        let result = node
            .build_vks(Some(Either::Left(indices.iter().copied().collect())), args.chunk)
            .await
            .context("build_vks")?;
        let build_seconds = t.elapsed().as_secs_f64();
        if !result.panic_indices.is_empty() {
            eprintln!(
                "WARNING: build_vks panicked on {} shape indices: {:?}",
                result.panic_indices.len(),
                result.panic_indices
            );
            anyhow::ensure!(
                args.allow_panics,
                "build_vks panicked on {} shape indices; pass --allow-panics true to accept a \
                 map without them (its root differs from a clean build's)",
                result.panic_indices.len()
            );
        }

        let map_bytes = bincode::serialize(&result.vk_map).context("serialize the vk map")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &map_bytes).context("write the vk map")?;
        let report_json = serde_json::json!({
            "mode": "pipeline-local",
            "proof_mode": "VkMapBlock",
            "chip_clusters": clusters,
            "preprocessed_multiple": args.block,
            "block_shapes": block_len,
            "block_first_index": block[0],
            "block_last_index": block[block_len - 1],
            "sampled": args.sample,
            "slice_index": args.slice_index,
            "slice_count": args.slice_count,
            "programs_built": indices.len(),
            "shape_space": shapes.len(),
            "vk_map_keys": result.vk_map.len(),
            "vk_map_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&map_bytes)),
            "vk_map_path": path.display().to_string(),
            "panic_indices": result.panic_indices,
            "chunk": args.chunk,
            "seconds": { "build_vks": build_seconds },
            "seconds_per_program": build_seconds / indices.len() as f64,
            "complete": args.sample.is_none() && args.slice_index.is_none(),
            "note": "covers every shape an input to a program in this preprocessed block can reach, so the root does not move with the input",
        });
        if let Some(parent) = args.output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&args.output, format!("{}\n", serde_json::to_string_pretty(&report_json)?))?;
        println!(
            "{} keys in {:.0}s ({:.3}s/program) -> {}",
            result.vk_map.len(),
            build_seconds,
            build_seconds / indices.len() as f64,
            path.display()
        );
        return Ok(());
    }

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
        indices.extend(shapes.len() - FIXED_TAIL..shapes.len());

        let t = Instant::now();
        let result = node
            .build_vks(Some(Either::Left(indices.iter().copied().collect())), args.chunk)
            .await
            .context("build_vks")?;
        let build_seconds = t.elapsed().as_secs_f64();
        if !result.panic_indices.is_empty() {
            eprintln!(
                "WARNING: build_vks panicked on {} shape indices: {:?}",
                result.panic_indices.len(),
                result.panic_indices
            );
            anyhow::ensure!(
                args.allow_panics,
                "build_vks panicked on {} shape indices; pass --allow-panics true to accept a \
                 map without them (its root differs from a clean build's)",
                result.panic_indices.len()
            );
        }

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
            "panic_indices": result.panic_indices,
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

    // The wrap proof, for a later `--from-wrap`. `shrink_wrap` takes a
    // compressed proof and refuses anything else
    // (`worker/node/full/mod.rs:259`), which is why this mode proves
    // compressed rather than groth16.
    let t = Instant::now();
    let (wrap_bytes, wrap_seconds) = if matches!(args.mode, Mode::Wrap) {
        let wrapped = node.shrink_wrap(&proof.proof).await.context("shrink wrap")?;
        (
            Some(bincode::serialize(&wrapped).context("serialize the wrap proof")?),
            t.elapsed().as_secs_f64(),
        )
    } else {
        (None, 0.0)
    };
    if let Some(bytes) = &wrap_bytes {
        let path = args.wrap_out.clone().context("--mode wrap needs --wrap-out <path>")?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, bytes).context("write the wrap proof")?;
        println!("wrap proof: {} bytes -> {}", bytes.len(), path.display());
    }

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
        "wrap_proof_bytes": wrap_bytes.as_ref().map(|b| b.len()),
        "seconds": {
            "node_build": node_built.as_secs_f64(),
            "execute": execute_seconds,
            "setup": setup_seconds,
            "prove": prove_seconds,
            "verify": verify_seconds,
            "shrink_wrap": wrap_seconds,
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
