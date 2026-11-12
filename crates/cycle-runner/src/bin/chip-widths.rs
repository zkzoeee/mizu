//! Prints every RiscvAir chip's registered cost and width decomposition.
//!
//! The project's trace accounting computes a chip's main cells as
//! `rows * (cost() - preprocessed_width())`
//! (`tools/pasta-proof/src/trace_accounting.rs`). Cycles stop being a proxy
//! for proving cost once work moves into a precompile's AIR, so the mirror
//! benchmark needs these widths to turn its permutation counts into cells.
//! This reads them from the fork itself rather than from a document.
//!
//! No execution, no records, no proof: it inspects the machine's chip list.
use sp1_core_machine::riscv::RiscvAir;
use sp1_hypercube::air::MachineAir;
use sp1_primitives::SP1Field;

fn main() {
    let chips = RiscvAir::<SP1Field>::chips();
    println!("{:<34} {:>10} {:>12} {:>11}", "chip", "cost", "preprocessed", "main_width");
    let mut rows: Vec<(&str, u64, u64, u64)> = chips
        .iter()
        .map(|chip| {
            let name = <_ as MachineAir<SP1Field>>::name(chip);
            let cost = chip.cost();
            let prep = <_ as MachineAir<SP1Field>>::preprocessed_width(chip) as u64;
            (name, cost, prep, cost.saturating_sub(prep))
        })
        .collect();
    rows.sort_by(|a, b| b.3.cmp(&a.3));
    for (name, cost, prep, main) in &rows {
        println!("{name:<34} {cost:>10} {prep:>12} {main:>11}");
    }
}
