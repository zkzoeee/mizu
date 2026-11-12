//! How long would the real recursion VK map take to build?
//!
//! Phase A task 3 is the critical path: `vk_verification = true` needs the real
//! map, and that gates matching upstream's wrap vk, hence ceremony-free Groth16,
//! and soundness generally (every proof so far carries
//! `without_vk_verification()`).
//!
//! `build_vks` takes an explicit index list (`Either::Left`), so the cost can be
//! sampled instead of guessed. This builds a strided sample across the shape
//! space -- strided, not the first N, because per-program cost varies with the
//! shape's preprocessed/main multiples -- times it, and extrapolates.
//!
//! Two counts matter and they differ:
//!   * `create_all_input_shapes(..).len()` is how many programs get BUILT.
//!   * its dedup'd length is how many KEYS the map ends up with, since shapes
//!     shared between clusters produce the same vk (`recursion.rs:63-67`).
//! Extrapolation uses the former.
//!
//! The wall-clock extrapolation assumes throughput scales with cores, which is
//! optimistic: `build_vks` has a serial controller and a fixed chunk size. Treat
//! it as an order of magnitude, which is all that is needed to size a rental.
use std::collections::BTreeSet;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use either::Either;
use sp1_core_machine::riscv::RiscvAir;
use sp1_prover::{
    shapes::create_all_input_shapes,
    worker::{cpu_worker_builder_with_machine, SP1LocalNodeBuilder},
};

/// `prover/src/shapes.rs:71`.
const DEFAULT_ARITY: usize = 4;

#[derive(Parser)]
struct Args {
    /// How many programs to build in the sample. Start small.
    #[arg(long, default_value = "8")]
    count: usize,
    /// `build_vks` chunk size.
    #[arg(long, default_value = "4")]
    chunk: usize,
    /// Cores assumed for the extrapolated wall-clock figure.
    #[arg(long, default_value = "32")]
    target_cores: f64,
    /// Cores this sample effectively had.
    #[arg(long, default_value = "1")]
    sample_cores: f64,
    #[arg(long)]
    output: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let machine = RiscvAir::machine();
    let clusters = machine.shape().chip_clusters.len();

    let shapes = create_all_input_shapes(machine.shape(), DEFAULT_ARITY);
    let to_build = shapes.len();
    let keys = shapes.iter().cloned().collect::<BTreeSet<_>>().len() + DEFAULT_ARITY + 2;

    // Strided, so the sample spans small and large shapes alike.
    let stride = (to_build / args.count).max(1);
    let indices: Vec<usize> = (0..args.count).map(|i| (i * stride).min(to_build - 1)).collect();

    println!("chip clusters        {clusters}");
    println!("programs to build    {to_build}");
    println!("resulting map keys   {keys}");
    println!("sample               {} programs, stride {stride}, chunk {}", indices.len(), args.chunk);
    println!("building...");

    let node = SP1LocalNodeBuilder::from_worker_client_builder(cpu_worker_builder_with_machine(
        machine,
    ))
    .build()
    .await
    .context("build the local node")?;

    let t = Instant::now();
    let out = node
        .build_vks(Some(Either::Left(indices.clone())), args.chunk)
        .await
        .context("build_vks on the sample")?;
    let elapsed = t.elapsed().as_secs_f64();

    let per = elapsed / indices.len() as f64;
    let total_core_seconds = per * to_build as f64 * args.sample_cores;
    let projected_wall_hours = total_core_seconds / args.target_cores / 3600.0;

    println!("\nsample elapsed       {elapsed:.1} s");
    println!("per program          {per:.3} s at {} core(s)", args.sample_cores);
    println!("full build           {:.1} core-hours", total_core_seconds / 3600.0);
    println!(
        "  projected wall     {projected_wall_hours:.2} h on {} cores",
        args.target_cores
    );
    println!(
        "  at $1.76/hr        ${:.2}  (32-core cpu3m memory-optimized pod)",
        projected_wall_hours * 1.76
    );
    println!("\nvks returned by the sample: {}", out.vk_map.len());

    if let Some(path) = args.output {
        let report = serde_json::json!({
            "mode": "vk-map-sample",
            "chip_clusters": clusters,
            "programs_to_build": to_build,
            "resulting_map_keys": keys,
            "sample_size": indices.len(),
            "stride": stride,
            "chunk": args.chunk,
            "sample_seconds": elapsed,
            "seconds_per_program": per,
            "sample_cores": args.sample_cores,
            "full_build_core_hours": total_core_seconds / 3600.0,
            "projected_wall_hours": projected_wall_hours,
            "target_cores": args.target_cores,
            "caveat": "wall-clock assumes throughput scales with cores; build_vks has a serial controller, so this is an order of magnitude, not a quote",
        });
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&report)?))?;
    }
    Ok(())
}
