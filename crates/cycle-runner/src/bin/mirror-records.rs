//! CPU-only record accounting for the mirror guest: per-chip rows, main cells
//! and global interaction events, from generated execution records rather than
//! from widths and assumed row counts.
//!
//! Why this exists: once Poseidon and keccak move into precompiles, the cycle
//! count stops tracking proving cost, and the width-derived cell estimate in
//! `benchmarks/mirror/RESULTS.md` omits the CPU chips the schedule still
//! touches around each syscall, and omits interaction events entirely. This
//! closes both gaps.
//!
//! The row and cell decomposition is the one `tools/pasta-proof`'s trace
//! accounting uses: for every chip included in a record,
//! `main_cells = num_rows(chip, record) * (cost() - preprocessed_width())`.
//! Unlike that tool this does NOT cross-check against `shape_from_record`, so
//! it reports stored cells without the stacking-height rounding or padding
//! columns, and it generates no proof of any kind.
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use sha2::{Digest, Sha256};
use sp1_core_executor::{Program, RetainedEventsPreset, SP1CoreOpts, ShardingThreshold};
use sp1_core_machine::{
    io::SP1Stdin,
    riscv::RiscvAir,
    utils::generate_records_streaming_with_summary,
};
use sp1_hypercube::air::MachineAir;
use sp1_primitives::SP1Field;
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc, time::Instant};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    elf: PathBuf,
    #[arg(long, value_parser=clap::value_parser!(u32).range(0..=4))]
    mode: u32,
    #[arg(long, default_value_t = 1)]
    accounts: u32,
    #[arg(long, default_value_t = 35)]
    depth: u32,
    #[arg(long)]
    output: PathBuf,
}

/// The same pinned preset the project's trace accounting uses, so shard areas
/// are comparable with its records and not with ambient SP1 defaults.
/// `PASTA_RETAIN`, mapped exactly as the GPU harness maps it
/// (`tools/pasta-proof/src/main.rs:2407`): `uint` retains `UINT256_MUL`,
/// `pasta-uint` adds both Pasta additions. Unset keeps default routing.
fn apply_retention_env(opts: &mut SP1CoreOpts) {
    match std::env::var("PASTA_RETAIN").as_deref() {
        Ok("uint") => {
            opts.retained_events_presets.insert(RetainedEventsPreset::Uint256MulMod);
        }
        Ok("pasta-uint") => {
            opts.retained_events_presets.insert(RetainedEventsPreset::Uint256MulMod);
            opts.retained_events_presets.insert(RetainedEventsPreset::VestaAdd);
            opts.retained_events_presets.insert(RetainedEventsPreset::PallasAdd);
        }
        Ok(other) => panic!("unknown PASTA_RETAIN routing {other:?}"),
        Err(_) => {}
    }
}

fn record_options() -> SP1CoreOpts {
    let mut opts = SP1CoreOpts {
        minimal_trace_chunk_threshold: 1 << 24,
        trace_chunk_slots: 1,
        memory_limit: 2 * 1024 * 1024 * 1024,
        shard_size: 1 << 24,
        sharding_threshold: ShardingThreshold {
            element_threshold: sp1_core_executor::ELEMENT_THRESHOLD,
            height_threshold: sp1_core_executor::HEIGHT_THRESHOLD,
        },
        global_dependencies_opt: true,
        ..SP1CoreOpts::default()
    };
    apply_retention_env(&mut opts);
    opts
}

#[derive(Default)]
struct ChipTotals {
    rows: u64,
    main_width: u64,
    prep_width: u64,
    shards: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let elf = fs::read(&args.elf).context("read mirror guest")?;
    let program = Arc::new(
        Program::from(elf.as_slice()).map_err(|e| anyhow!("disassemble mirror guest: {e}"))?,
    );
    let mut stdin = SP1Stdin::new();
    stdin.write(&(args.mode, args.accounts, args.depth));

    let chips = RiscvAir::<SP1Field>::chips();
    let mut totals: BTreeMap<String, ChipTotals> = BTreeMap::new();
    let mut records = 0u64;
    let mut cpu_events = 0u64;
    let mut interactions = 0u64;
    let mut memory_init = 0u64;
    let mut memory_final = 0u64;
    let mut failure: Option<String> = None;

    let started = Instant::now();
    let summary = generate_records_streaming_with_summary::<SP1Field, _>(
        program,
        stdin,
        record_options(),
        [0; 4],
        |record| {
            records += 1;
            cpu_events += record.cpu_event_count as u64;
            interactions += record.global_interaction_events.len() as u64;
            memory_init += record.global_memory_initialize_events.len() as u64;
            memory_final += record.global_memory_finalize_events.len() as u64;
            for chip in chips.iter() {
                if !chip.included(&record) {
                    continue;
                }
                let name = <_ as MachineAir<SP1Field>>::name(chip).to_string();
                let rows = <_ as MachineAir<SP1Field>>::num_rows(chip, &record).unwrap_or(0) as u64;
                let prep = <_ as MachineAir<SP1Field>>::preprocessed_width(chip) as u64;
                let Some(main) = chip.cost().checked_sub(prep) else {
                    failure = Some(format!("chip {name} cost below preprocessed width"));
                    return false;
                };
                let entry = totals.entry(name).or_default();
                entry.rows += rows;
                entry.shards += 1;
                entry.main_width = main;
                entry.prep_width = prep;
            }
            true
        },
    )
    .map_err(|e| anyhow!("record execution: {e}"))?;
    if let Some(error) = failure {
        anyhow::bail!("{error}");
    }

    let main_cells: u64 = totals.values().map(|t| t.rows * t.main_width).sum();
    let prep_cells: u64 = totals.values().map(|t| t.rows * t.prep_width).sum();
    let mut by_chip: Vec<_> = totals
        .iter()
        .map(|(name, t)| {
            serde_json::json!({
                "chip": name, "rows": t.rows, "main_width": t.main_width,
                "preprocessed_width": t.prep_width, "main_cells": t.rows * t.main_width,
                "shards_included": t.shards,
            })
        })
        .collect();
    by_chip.sort_by_key(|v| std::cmp::Reverse(v["main_cells"].as_u64().unwrap_or(0)));

    let report = serde_json::json!({
        "mode": "cpu-mirror-records",
        "pasta_retain": std::env::var("PASTA_RETAIN").unwrap_or_else(|_| "default".into()),
        "proof_generated": false,
        "mirror_mode": args.mode,
        "accounts": args.accounts,
        "depth": args.depth,
        "elf_sha256": hex::encode(Sha256::digest(&elf)),
        "records": records,
        "cycles": summary.cycles,
        "cpu_instruction_events": cpu_events,
        "global_interaction_events": interactions,
        "global_memory_initialize_events": memory_init,
        "global_memory_finalize_events": memory_final,
        "main_stored_cells": main_cells,
        "preprocessed_stored_cells": prep_cells,
        "chips": by_chip,
        "note": "stored cells only: no shape_from_record cross-check, so no stacking-height rounding or padding columns. Main cells are prover inputs, not proving time.",
        "elapsed_seconds": started.elapsed().as_secs_f64(),
    });
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &args.output,
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    println!(
        "mode {} n={} depth {}: {} records, {} cycles, {} main cells, {} interaction events",
        args.mode, args.accounts, args.depth, records, summary.cycles, main_cells, interactions
    );
    Ok(())
}
