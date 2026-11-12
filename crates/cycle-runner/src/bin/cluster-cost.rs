//! Stage 4 of `docs/groth16-ceremony-spike.md`: what does the lean cluster trim
//! actually cost?
//!
//! The answer is *not* wasted rows. `MachineShape::smallest_cluster` picks the
//! smallest cluster containing a shard's chip set (`prover/trace.rs:157`), and a
//! chip that is in the chosen cluster but absent from the record gets
//!
//! ```text
//! PaddedMle::zeros(chip.width(), max_log_row_count)
//! ```
//!
//! whose `inner` is `None`, so `num_real_entries()` is 0
//! (`slop/multilinear/src/padded.rs:64-125`). The jagged prover's row counts are
//! exactly those (`slop/jagged/src/prover.rs:110`), so an event-free chip
//! contributes **zero rows** to the commitment. There are no wasted cells.
//!
//! What the trim does cost is per-chip constant overhead: each extra chip in the
//! cluster is one more named trace, one more row-count entry and one more set of
//! constraint evaluations for prover and verifier. That cost scales with the
//! *number* of extra chips, so this tool measures exactly that: for every cluster
//! the full machine offers, what would the lean machine select instead, and how
//! many more chips does that pull in?
//!
//! Each cluster is used as a hypothetical shard chip set, which is the tightest
//! realistic case: a shard whose chips exactly fill a full-machine cluster is
//! precisely the shard that loses the most when that cluster disappears.
//!
//! No proving, no records, no network.
use std::collections::{BTreeMap, BTreeSet};

use sp1_core_machine::riscv::RiscvAir;
use sp1_hypercube::air::MachineAir;
use sp1_primitives::SP1Field;

const GATE: &str = "MIZU_LEAN_PASTA_CLUSTERS";

fn cluster_names(
    machine: &sp1_hypercube::Machine<SP1Field, RiscvAir<SP1Field>>,
) -> Vec<BTreeSet<String>> {
    machine
        .shape()
        .chip_clusters
        .iter()
        .map(|c| c.iter().map(|chip| chip.name().to_string()).collect())
        .collect()
}

fn main() {
    // Build both machines in one process: machine() reads the gate at
    // construction, and this binary is single threaded.
    std::env::remove_var(GATE);
    let full = RiscvAir::<SP1Field>::machine();
    let full_clusters = cluster_names(&full);
    drop(full);

    std::env::set_var(GATE, "1");
    let lean = RiscvAir::<SP1Field>::machine();
    let lean_clusters = cluster_names(&lean);
    let lean_chips: Vec<_> = lean.chips().to_vec();

    println!("full machine: {} clusters", full_clusters.len());
    println!("lean machine: {} clusters", lean_clusters.len());
    println!(
        "\nrows wasted by the trim: 0 by construction -- an event-free chip in the\n\
         chosen cluster is PaddedMle::zeros with inner=None, so num_real_entries()==0\n\
         and the jagged prover's row count for it is 0.\n\
         What follows is the only real cost: extra chips per affected shard.\n"
    );

    // For each cluster the full machine offers, what does lean pick instead?
    let mut deltas: BTreeMap<usize, usize> = BTreeMap::new();
    let mut worst: Vec<(usize, usize, usize, String)> = Vec::new();
    let mut unsatisfiable = 0usize;

    for (i, want) in full_clusters.iter().enumerate() {
        // Rebuild the chip set against the lean machine's chip objects.
        let chip_set: BTreeSet<_> =
            lean_chips.iter().filter(|c| want.contains(c.name())).cloned().collect();
        if chip_set.len() != want.len() {
            // A chip the lean machine does not have at all: cannot happen here
            // (the trim removes clusters, never chips), so flag it loudly.
            println!("  cluster {i}: {} of {} chips missing from the lean machine", want.len() - chip_set.len(), want.len());
            unsatisfiable += 1;
            continue;
        }
        match lean.smallest_cluster(&chip_set) {
            None => {
                println!("  cluster {i} ({} chips): NO lean cluster covers it", want.len());
                unsatisfiable += 1;
            }
            Some(got) => {
                let delta = got.len() - want.len();
                *deltas.entry(delta).or_default() += 1;
                if delta > 0 {
                    let sample = want
                        .iter()
                        .filter(|n| {
                            n.contains("Pallas") || n.contains("Vesta") || n.contains("Uint256")
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(",");
                    worst.push((i, want.len(), got.len(), sample));
                }
            }
        }
    }

    println!("extra chips per shard, over all {} full-machine clusters:", full_clusters.len());
    for (delta, count) in &deltas {
        println!("  +{delta:<3} chips  {count:>3} clusters");
    }
    if unsatisfiable > 0 {
        println!("  UNSATISFIABLE: {unsatisfiable} -- the lean set cannot prove these shards");
    }

    if worst.is_empty() {
        println!("\nNo cluster loses anything: the trim is free even in per-chip overhead.");
    } else {
        println!("\nAffected clusters (pasta/uint chips they contain):");
        worst.sort_by_key(|(_, want, got, _)| std::cmp::Reverse(got - want));
        for (i, want, got, sample) in worst.iter().take(16) {
            println!("  [{i:>3}] {want} -> {got} chips (+{})  {sample}", got - want);
        }
        let max = worst.iter().map(|(_, w, g, _)| g - w).max().unwrap();
        println!(
            "\nWorst case: +{max} zero-row chips on an affected shard. Cost is {max} extra\n\
             named traces / row-count entries / constraint evaluations, and zero rows."
        );
    }
}
