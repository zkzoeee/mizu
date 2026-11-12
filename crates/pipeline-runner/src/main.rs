//! Milestone 1 of the proof pipeline: core -> compress -> shrink -> wrap, with
//! the recursion VK map stubbed.
//!
//! Two switches in the fork make this runnable before any recursion program is
//! compiled, both documented in docs/proof-pipeline-plan.md:
//!
//!   build_vk_map(dummy: true, ..)          shapes.rs:485  - no compilation
//!   VerifierRecursionVks { vk_verification: false }
//!                                          verify.rs:578  - no membership check
//!
//! SP1's own comment on the second: "This `vk_verification` must be only turned
//! off for testing purposes." Anything this binary produces is a development
//! artifact: with a dummy map nothing binds the recursion program that made the
//! proof, so a prover could substitute a different one. Label its outputs.
//!
//! This file is a compile gate for now. It establishes that the fork's prover
//! resolves and builds from the root workspace; the stages come next.
use anyhow::Result;
use sp1_core_machine::riscv::RiscvAir;
use sp1_hypercube::air::MachineAir;
use sp1_primitives::SP1Field;

fn main() -> Result<()> {
    // Touch the fork's machine through the patched graph, so a mixed
    // crates.io/fork resolution fails here rather than deep in the prover.
    let machine = RiscvAir::<SP1Field>::machine();
    let clusters = machine.shape().chip_clusters.len();
    let chips = machine.chips().len();
    let pasta = machine
        .chips()
        .iter()
        .filter(|c| {
            let n = <_ as MachineAir<SP1Field>>::name(*c);
            n.contains("Pallas") || n.contains("Vesta")
        })
        .count();
    println!("fork machine resolved: {chips} chips, {clusters} clusters, {pasta} Pasta chips");
    println!("next: compress -> shrink -> wrap with dummy vk map and vk_verification off");
    Ok(())
}
