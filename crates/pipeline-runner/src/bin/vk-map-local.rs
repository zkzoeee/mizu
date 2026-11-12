//! Build the partial recursion VK map for one workload **locally**, without
//! proving.
//!
//! `--mode vk-map` on the drivers gets the shard shapes from a core proof's
//! shards (`shape_from_proof`). A core proof needs a GPU, or more than 64 GiB on
//! CPU. But the shape of a shard is a function of its *record*, not of its
//! proof: `shape_from_record` (`hypercube/src/prover/simple.rs:20`) derives the
//! chip set and the two stacked areas from the record alone, and that is what
//! `tools/pasta-proof`'s trace accounting already does locally. So the records
//! can be streamed on a laptop, each dropped as soon as its shape is taken, and
//! the vks built from the resulting index set.
//!
//! What this is for: deciding the guest and retention configuration, then
//! building the map it implies, before renting anything. The map's root is a
//! public input of the Groth16 proof and is pinned in the verifier contract, so
//! it has to be settled before deployment.
//!
//! ## The map is only valid for a prover that shards the same way
//!
//! Shard shapes depend on `SP1CoreOpts`, so this mirrors what the GPU prover
//! uses (`sp1-gpu-prover` `local_gpu_opts`, builder.rs:26): `shard_size = 1 <<
//! 24`, `global_dependencies_opt`, and the full `ELEMENT_THRESHOLD`.
//!
//! That last one is conditional upstream:
//!
//! ```text
//! gpu_memory_gb = ceil(vram / GB) + 4
//! threshold = if !full_size_shards && gpu_memory_gb <= 30 { ELEMENT_THRESHOLD - 2^26 - 2^25 - 2^24 }
//!             else { ELEMENT_THRESHOLD }
//! ```
//!
//! So a card with more than about 26 GB of VRAM (A40/A6000 at 48 GB, 5090 at 32)
//! takes the full threshold and matches this tool. **A 24 GB card — A5000, 3090,
//! 4090 — shards differently and this map will not fit it.** The report records
//! the assumption.
//!
//! A missing shape is not silent: with `vk_verification` on, a shard whose vk is
//! absent from the map fails the membership check.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result};
use clap::Parser;
use either::Either;
use sp1_core_executor::{Program, RetainedEventsPreset, SP1CoreOpts, ELEMENT_THRESHOLD};
use sp1_core_machine::{
    io::SP1Stdin, riscv::RiscvAir, utils::generate_records_streaming_with_summary,
};
use sp1_hypercube::prover::shape_from_record;
use sp1_primitives::SP1Field;
use sp1_prover::{
    shapes::{create_all_input_shapes, SP1RecursionProgramShape},
    worker::{cpu_worker_builder_with_machine, SP1LocalNodeBuilder},
    CpuSP1ProverComponents, SP1ProverComponents,
};

/// `prover/src/shapes.rs:71`.
const DEFAULT_ARITY: usize = 4;

#[derive(Parser)]
struct Args {
    /// The guest ELF the map is for. A different ELF shards differently.
    #[arg(long)]
    elf: PathBuf,
    /// Directory holding `proof-input.bin` and `receipt-input.bin`.
    #[arg(long)]
    input_dir: PathBuf,
    /// Where to write the map, for `--vk-map` on either driver.
    #[arg(long)]
    vk_map: PathBuf,
    #[arg(long)]
    output: PathBuf,
    /// Retain the Pasta and UInt256 events instead of deferring them, as the
    /// `pasta-proof` harness's retained arm does. On by default: it is the
    /// configuration this project proves with.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    retain_pasta: bool,
    /// `build_vks` chunk size. Each program costs seconds of CPU, so keep this
    /// small on a laptop.
    #[arg(long, default_value = "4")]
    chunk: usize,
    /// Stop after this many records, for a smoke test. The resulting map is
    /// incomplete and the report says so.
    #[arg(long)]
    max_records: Option<usize>,
}

/// Mirrors the shard-shaping fields of `local_gpu_opts` for a card above the
/// 30 GB branch, plus this project's retention.
fn opts_for(retain_pasta: bool) -> SP1CoreOpts {
    let mut opts = SP1CoreOpts::default();
    opts.shard_size = 1 << 24;
    opts.sharding_threshold.element_threshold = ELEMENT_THRESHOLD;
    opts.global_dependencies_opt = true;
    if retain_pasta {
        opts.retained_events_presets.insert(RetainedEventsPreset::Uint256MulMod);
        opts.retained_events_presets.insert(RetainedEventsPreset::VestaAdd);
        opts.retained_events_presets.insert(RetainedEventsPreset::PallasAdd);
    }
    opts
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let elf = fs::read(&args.elf).context("read guest ELF")?;
    let proof_input =
        fs::read(args.input_dir.join("proof-input.bin")).context("read proof-input.bin")?;
    let receipt_input =
        fs::read(args.input_dir.join("receipt-input.bin")).context("read receipt-input.bin")?;
    let mut stdin = SP1Stdin::new();
    stdin.write_slice(&proof_input);
    stdin.write_slice(&receipt_input);

    let machine = RiscvAir::<SP1Field>::machine();
    let clusters = machine.shape().chip_clusters.len();
    // The prover's own verifier, not a reconstructed one, so no parameter drift.
    let core_verifier = CpuSP1ProverComponents::core_verifier(machine.clone());
    let opts = opts_for(args.retain_pasta);
    let mut presets: Vec<String> =
        opts.retained_events_presets.iter().map(|p| format!("{p:?}")).collect();
    presets.sort();

    println!("clusters {clusters}, retain_pasta {}", args.retain_pasta);
    println!("retained presets: {}", presets.join(", "));
    println!("streaming records (no proving)...");

    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("ELF decode: {e}"))?);
    let shapes = create_all_input_shapes(core_verifier.shard_verifier().machine().shape(), DEFAULT_ARITY);

    let mut records = 0usize;
    let mut distinct: BTreeMap<usize, usize> = BTreeMap::new(); // shape index -> record count
    let mut unknown = 0usize;
    let mut progress = Instant::now();
    let t = Instant::now();
    let summary = generate_records_streaming_with_summary::<SP1Field, _>(
        program,
        stdin,
        opts.clone(),
        [0; 4],
        |record| {
            records += 1;
            match shape_from_record(&core_verifier, &record) {
                Some(core_shape) => {
                    let shape = SP1RecursionProgramShape::Normalize(core_shape);
                    match shapes.iter().position(|s| s == &shape) {
                        Some(i) => *distinct.entry(i).or_default() += 1,
                        None => unknown += 1,
                    }
                }
                None => unknown += 1,
            }
            // The record is dropped here: memory stays bounded.
            if progress.elapsed().as_secs() >= 30 {
                println!("  {records} records, {} distinct shapes", distinct.len());
                progress = Instant::now();
            }
            !args.max_records.is_some_and(|cap| records >= cap)
        },
    )
    .map_err(|e| anyhow::anyhow!("record execution: {e}"))?;
    let record_seconds = t.elapsed().as_secs_f64();
    anyhow::ensure!(unknown == 0, "{unknown} records had no shape in create_all_input_shapes");

    let mut indices: BTreeSet<usize> = distinct.keys().copied().collect();
    let shard_shapes = indices.len();
    indices.extend(shapes.len() - 12..shapes.len());

    println!(
        "{records} records, {shard_shapes} distinct shard shapes, {} programs to build, {:.0}s to stream",
        indices.len(),
        record_seconds
    );
    println!("building vks (seconds each on CPU)...");

    let node = SP1LocalNodeBuilder::from_worker_client_builder(cpu_worker_builder_with_machine(
        machine,
    ))
    .build()
    .await
    .context("build the local node")?;
    let t = Instant::now();
    let result = node
        .build_vks(Some(Either::Left(indices.iter().copied().collect())), args.chunk)
        .await
        .context("build_vks")?;
    let build_seconds = t.elapsed().as_secs_f64();
    anyhow::ensure!(
        result.panic_indices.is_empty(),
        "build_vks panicked on shape indices {:?}",
        result.panic_indices
    );

    let map_bytes = bincode::serialize(&result.vk_map).context("serialize the vk map")?;
    if let Some(parent) = args.vk_map.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.vk_map, &map_bytes).context("write the vk map")?;

    let report = serde_json::json!({
        "mode": "vk-map-local",
        "elf": args.elf.display().to_string(),
        "elf_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&elf)),
        "cycles": summary.cycles,
        "complete": !summary.stopped_early,
        "chip_clusters": clusters,
        "retain_pasta": args.retain_pasta,
        "retained_events_presets": presets,
        "records": records,
        "distinct_shard_shapes": shard_shapes,
        "records_per_shape": distinct.values().copied().collect::<Vec<_>>(),
        "programs_built": indices.len(),
        "shape_indices": indices,
        "shape_space": shapes.len(),
        "vk_map_keys": result.vk_map.len(),
        "vk_map_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&map_bytes)),
        "vk_map_path": args.vk_map.display().to_string(),
        "seconds": { "stream_records": record_seconds, "build_vks": build_seconds },
        "seconds_per_program": build_seconds / indices.len() as f64,
        "sharding_opts": {
            "shard_size": opts.shard_size,
            "element_threshold": opts.sharding_threshold.element_threshold,
            "global_dependencies_opt": opts.global_dependencies_opt,
            "full_size_shards": opts.full_size_shards,
            "mirrors": "sp1-gpu-prover local_gpu_opts for a card above its 30 GB branch",
        },
        "validity": "only for a prover sharding identically: VRAM above ~26 GB (A40/A6000/5090). A 24 GB card takes local_gpu_opts' reduced element_threshold and will not fit this map.",
        "on_chain": "the map's root is a Groth16 public input and is pinned as VK_ROOT in the verifier contract, so settle the guest and retention config before deploying",
    });
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.output, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    println!(
        "{} keys in {:.0}s ({:.2}s/program) -> {}",
        result.vk_map.len(),
        build_seconds,
        build_seconds / indices.len() as f64,
        args.vk_map.display()
    );
    Ok(())
}
