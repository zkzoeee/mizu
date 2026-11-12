//! Phase A task 1: how many recursion programs does this fork's machine imply?
//!
//! `sp1-prover`'s `create_all_input_shapes` (crates/prover/src/shapes.rs:702)
//! enumerates one `Normalize` recursion program per
//!
//!   chip cluster x preprocessed_multiple x main_multiple
//!                x main_padding_cols x preprocessed_padding_cols
//!
//! and then one `Compose` per arity, plus `Deferred` and `Shrink`. The VK map
//! that recursion needs covers exactly that set, so its size — and therefore
//! the cost of building it — is `clusters * shapes_per_cluster + arity + 2`.
//!
//! Adding the Pasta precompiles to the core machine adds clusters, which
//! multiplies straight through. This prints the count, and which clusters are
//! ours, so Phase A can be scoped before any GPU time is bought.
//!
//! Reads the fork's own machine. No proving, no recursion, no network.
use sp1_core_executor::{ELEMENT_THRESHOLD, MAX_PROGRAM_SIZE};
use sp1_core_machine::{
    bytes::columns::NUM_BYTE_PREPROCESSED_COLS, program::NUM_PROGRAM_PREPROCESSED_COLS,
    range::columns::NUM_RANGE_PREPROCESSED_COLS, riscv::RiscvAir,
};
use sp1_hypercube::air::MachineAir;
use sp1_primitives::SP1Field;

/// `crates/prover/src/components.rs:16-17`.
const CORE_LOG_STACKING_HEIGHT: u32 = 21;
const CORE_MAX_LOG_ROW_COUNT: usize = 22;
/// `crates/prover/src/shapes.rs:71`.
const DEFAULT_ARITY: usize = 4;
/// `crates/prover/src/shapes.rs:94`.
fn padded_element_threshold() -> u64 {
    ELEMENT_THRESHOLD + (1 << CORE_LOG_STACKING_HEIGHT)
}

/// `crates/prover/src/shapes.rs:697`.
fn max_main_multiple_for_preprocessed_multiple(preprocessed_multiple: usize) -> usize {
    (padded_element_threshold() - preprocessed_multiple as u64 * (1 << CORE_LOG_STACKING_HEIGHT))
        .div_ceil(1 << CORE_LOG_STACKING_HEIGHT as u64) as usize
}

/// `crates/prover/src/shapes.rs:741`, reproduced so this needs no sp1-prover.
fn normalize_program_parameter_space() -> (usize, usize, usize) {
    let max_preprocessed_multiple = (MAX_PROGRAM_SIZE * NUM_PROGRAM_PREPROCESSED_COLS
        + (1 << 17) * NUM_RANGE_PREPROCESSED_COLS
        + (1 << 16) * NUM_BYTE_PREPROCESSED_COLS)
        .div_ceil(1 << CORE_LOG_STACKING_HEIGHT);
    let max_main_multiple = padded_element_threshold().div_ceil(1 << CORE_LOG_STACKING_HEIGHT) as usize;
    let num_shapes = (0..=max_preprocessed_multiple)
        .map(max_main_multiple_for_preprocessed_multiple)
        .sum::<usize>();
    (max_preprocessed_multiple, max_main_multiple, num_shapes)
}

fn main() {
    // The REAL clustering, built in RiscvAir::machine() (riscv/mod.rs:1040) --
    // not MachineShape::all, which collapses every chip into one cluster and
    // badly undercounts.
    let machine = RiscvAir::<SP1Field>::machine();
    let chips = machine.chips();
    let clusters = &machine.shape().chip_clusters;

    let (max_prep, max_main, num_shapes) = normalize_program_parameter_space();
    let max_padding_cols = ((1usize << CORE_LOG_STACKING_HEIGHT) as usize)
        .div_ceil(1 << CORE_MAX_LOG_ROW_COUNT);
    // create_all_input_shapes iterates padding cols 1..=max for BOTH axes.
    let per_cluster = num_shapes * max_padding_cols * max_padding_cols;
    let normalize = clusters.len() * per_cluster;

    println!("chips in the machine                {}", chips.len());
    println!("mprotect feature                    {}", if cfg!(feature = "mprotect") { "on" } else { "off" });
    println!("chip clusters                       {}", clusters.len());
    println!("max preprocessed multiple           {max_prep}");
    println!("max main multiple                   {max_main}");
    println!("shapes from the multiple space       {num_shapes}");
    println!("max padding cols per axis           {max_padding_cols}");
    println!("shapes per cluster                  {per_cluster}");
    println!();
    println!("Normalize recursion programs        {normalize}");
    println!("Compose (arity 1..={DEFAULT_ARITY})                 {DEFAULT_ARITY}");
    println!("Deferred + Shrink                   2");
    println!("TOTAL recursion programs / VK map   {}", normalize + DEFAULT_ARITY + 2);
    println!();

    // Which clusters exist because of this fork's own precompiles?
    let ours = ["Pallas", "Vesta", "Uint256"];
    let mut fork_clusters = 0usize;
    println!("clusters containing a Pasta/Uint256 chip:");
    for (i, cluster) in clusters.iter().enumerate() {
        let names: Vec<&str> =
            cluster.iter().map(|c| <_ as MachineAir<SP1Field>>::name(c)).collect();
        if let Some(hit) = names.iter().find(|n| ours.iter().any(|o| n.contains(o))) {
            fork_clusters += 1;
            println!("  [{i:>3}] {:<28} ({} chips)", hit, names.len());
        }
    }
    println!();
    println!("fork-specific clusters              {fork_clusters} of {}", clusters.len());
    println!(
        "their share of the VK map           {} programs",
        fork_clusters * per_cluster
    );
}
