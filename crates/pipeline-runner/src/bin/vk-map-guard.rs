//! Guard: does this machine's recursion VK map still sit at upstream's depth?
//!
//! `allowed_vk_height` (`prover/src/worker/node/core.rs:92`) is
//! `num_keys().next_power_of_two().ilog2()`, the map is padded to a count
//! derived from `RiscvAir::machine().shape()` (`prover/src/recursion.rs:59-80`,
//! including on the `vk_verification = false` path), and that depth is baked
//! into the wrap circuit. Upstream sits at depth 18. Exceed it and our wrap vk
//! changes, upstream's released Groth16 artifacts stop applying, and a ceremony
//! becomes ours to run.
//!
//! Nothing in the pipeline checks this: the production artifact download keys by
//! `SP1_CIRCUIT_VERSION`, not the vk hash, so a mismatch is silent until a
//! verification failure. Hence a guard, so adding a chip cluster fails here
//! rather than in production.
//!
//! Calls the real `create_all_input_shapes` rather than reproducing its
//! enumeration. Exits non-zero when the invariant breaks.
//!
//! See docs/groth16-ceremony-spike.md.
use std::collections::BTreeSet;

use sp1_core_machine::riscv::RiscvAir;
use sp1_primitives::SP1Field;
use sp1_prover::shapes::create_all_input_shapes;

/// `prover/src/shapes.rs:71`.
const DEFAULT_ARITY: usize = 4;
/// What upstream's baked `vk_map.bin` implies, and what the wrap circuit expects.
const EXPECTED_DEPTH: u32 = 18;

fn main() {
    let machine = RiscvAir::<SP1Field>::machine();
    let clusters = machine.shape().chip_clusters.len();

    // Exactly what from_map does (prover/src/recursion.rs:63-67): the map is
    // padded to the number of DISTINCT input shapes, with nothing added.
    // `create_all_input_shapes` already emits the Compose-per-arity, Deferred and
    // Shrink shapes alongside the Normalize ones, so adding them again
    // double-counts -- which this tool did until 2026-11-13.
    let keys = create_all_input_shapes(machine.shape(), DEFAULT_ARITY)
        .into_iter()
        .collect::<BTreeSet<_>>()
        .len();
    let padded = keys.next_power_of_two();
    let depth = padded.trailing_zeros();

    let ceiling = 1usize << EXPECTED_DEPTH;
    println!("chip clusters            {clusters}");
    println!("padded VK map keys       {keys}");
    println!("next power of two        {padded} = 2^{depth}");
    println!("allowed_vk_height        {depth}  (upstream: {EXPECTED_DEPTH})");
    println!("ceiling for depth {EXPECTED_DEPTH}     {ceiling}");
    println!("headroom                 {} keys", ceiling.saturating_sub(keys));

    if depth != EXPECTED_DEPTH {
        eprintln!(
            "\nFAIL: depth {depth} != upstream's {EXPECTED_DEPTH}.\n\
             The wrap circuit changes, so upstream's released Groth16 artifacts no\n\
             longer apply and a ceremony becomes ours to run. Either reduce the chip\n\
             clusters until the padded count is <= {ceiling}, or accept the ceremony\n\
             deliberately and update docs/groth16-ceremony-spike.md."
        );
        std::process::exit(1);
    }
    println!("\nOK: depth {depth} matches upstream, so its released Groth16 artifacts apply.");
    if keys * 100 / ceiling > 95 {
        println!(
            "WARNING: {}% of the ceiling used. One more chip cluster (~6,001 keys)\n\
             would cross it.",
            keys * 100 / ceiling
        );
    }
}
