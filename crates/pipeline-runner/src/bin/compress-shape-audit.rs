//! Which chip clusters overflow upstream's compress shape, and does it matter
//! for the wrap verifying key?
//!
//! Two questions, both answered with SP1's own program builders and no proving.
//!
//! 1. **Per cluster**, what does the normalize program need? This mirrors
//!    `compute_compress_shape` (`prover/src/shapes.rs:198`), which is
//!    `#[cfg(test)]` and so unreachable: for each cluster, compile the normalize
//!    program at `create_test_shape(cluster)` and read its event counts; take
//!    the elementwise max; then grow until the arity-4 compose program fits its
//!    own shape. Splitting the max by "has a Pasta chip" / "has none" both
//!    validates the tool (the second should reproduce upstream's pinned
//!    `compress_shape.json`) and names the clusters responsible for any growth.
//!
//! 2. **The link nobody measured.** The wrap program is built from
//!    `shrink_shape.dummy_input(..)` (`worker/prover/recursion.rs:1165-1183`),
//!    and `dummy_input` sees a shape only through
//!    `ceil(sum(height * width) / 2^log_stacking_height)` (`shapes.rs:358-382`).
//!    So a changed shrink *program* does not by itself mean a changed wrap
//!    *input*: the shrink table heights have to cross a stacking multiple. For
//!    each compress shape this builds the shrink program, takes its real
//!    preprocessed table heights exactly as `preprocessed_table_heights` does
//!    (`hypercube/src/prover/shard.rs:363`), builds the wrap dummy input, and
//!    hashes it. Equal hashes mean an identical wrap program and wrap vk.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result};
use sp1_core_machine::riscv::RiscvAir;
use sp1_hypercube::{
    air::MachineAir,
    prover::{AirProver, DefaultTraceGenerator, ProverSemaphore, TraceGenerator},
};
use sp1_prover::{
    recursion::{
        compose_program_from_input, normalize_program_from_input, recursive_verifier,
        shrink_program_from_input, wrap_program_from_input,
    },
    shapes::{
        build_shape_from_recursion_air_event_count, create_test_shape, max_count,
        SP1RecursionProofShape,
    },
    verify::WRAP_VK_BYTES,
    worker::{cpu_worker_builder_with_machine, SP1LocalNodeBuilder},
    CpuSP1ProverComponents, CpuWrapProverBuilder, SP1ProverComponents, WrapProverBuilder,
};

const DEFAULT_ARITY: usize = 4;
/// Upstream's depth, and ours on the default (lean) cluster set.
const VK_MAP_HEIGHT: usize = 18;

/// `vendor/sp1/crates/prover/compress_shape.json` as upstream pins it (the copy
/// in this tree carries our Select bump).
fn upstream_heights() -> BTreeMap<String, usize> {
    [
        ("BaseAlu", 592_384),
        ("ExtAlu", 795_264),
        ("MemoryConst", 492_320),
        ("MemoryVar", 661_984),
        ("Poseidon2WideDeg3", 158_368),
        ("PrefixSumChecks", 224_608),
        ("PublicValues", 16),
        ("Select", 1_087_808),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

fn heights_of(shape: &SP1RecursionProofShape) -> BTreeMap<String, usize> {
    let v = serde_json::to_value(shape).expect("shape to json");
    serde_json::from_value(v["shape"]["heights"].clone()).expect("heights map")
}

fn shape_of(heights: &BTreeMap<String, usize>) -> SP1RecursionProofShape {
    serde_json::from_value(serde_json::json!({ "shape": { "heights": heights, "_marker": null } }))
        .expect("shape from heights")
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes))
}

/// The artifact directory prefix: first 8 bytes of sha256 over the bincode of the
/// vk (prover/src/build.rs:153-167).
fn vk_prefix(vk_bytes: &[u8]) -> String {
    let d = <sha2::Sha256 as sha2::Digest>::digest(vk_bytes);
    format!("{:016x}", u64::from_be_bytes(d[..8].try_into().unwrap()))
}

fn fmt_heights(h: &BTreeMap<String, usize>) -> String {
    h.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")
}

#[tokio::main]
async fn main() -> Result<()> {
    let elf_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "vendor/sp1/crates/prover/elf/riscv64im-succinct-zkvm-elf".to_string());
    let elf = std::fs::read(&elf_path).with_context(|| format!("read {elf_path}"))?;

    let machine = RiscvAir::machine();
    let clusters = machine.shape().chip_clusters.clone();

    // A real vk, as compute_compress_shape uses. Its values are witnessed, so
    // which program it belongs to does not shape the normalize program.
    let node = SP1LocalNodeBuilder::from_worker_client_builder(cpu_worker_builder_with_machine(
        machine.clone(),
    ))
    .build()
    .await
    .context("build the local node")?;
    let vk = node.setup(&elf).await.context("setup the small ELF")?;

    let core_verifier = CpuSP1ProverComponents::core_verifier(machine.clone());
    let rec_core = recursive_verifier(core_verifier.shard_verifier());

    let compress_verifier = CpuSP1ProverComponents::compress_verifier();
    let compress_chips: BTreeSet<_> =
        compress_verifier.shard_verifier().machine().chips().iter().cloned().collect();
    let c_mlrc = compress_verifier.max_log_row_count();
    let c_fri = *compress_verifier.fri_config();
    let c_lsh = compress_verifier.log_stacking_height() as usize;
    let rec_compress = recursive_verifier(compress_verifier.shard_verifier());

    let upstream = upstream_heights();

    // ---- 1. per-cluster normalize needs -------------------------------------
    println!("== per-cluster normalize program needs, at create_test_shape ==");
    println!(
        "{:>3} {:>5} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9}  {}",
        "idx", "chips", "cols", "ExtAlu", "PrefixSum", "Select", "BaseAlu", "MemVar", "notes"
    );
    let mut max_upstream_like = None;
    let mut max_fork = None;
    let t_all = Instant::now();
    for (i, cluster) in clusters.iter().enumerate() {
        // cost() is preprocessed_width + width (hypercube/src/chip.rs:154).
        let cols: usize =
            cluster.iter().map(|c| c.cost() as usize - c.preprocessed_width()).sum();
        let fork_chips: Vec<String> = cluster
            .iter()
            .map(|c| c.name().to_string())
            .filter(|n| n.contains("Pallas") || n.contains("Vesta"))
            .collect();
        let shape = create_test_shape(cluster);
        let program = normalize_program_from_input(&rec_core, &shape.dummy_input(vk.clone()));
        let ec = program.event_counts;
        let need = heights_of(&build_shape_from_recursion_air_event_count(&ec));
        let over: Vec<String> = need
            .iter()
            .filter(|(k, v)| upstream.get(*k).is_some_and(|u| *v > u))
            .map(|(k, v)| format!("{k}+{}", v - upstream[k]))
            .collect();
        println!(
            "{:>3} {:>5} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9}  {}{}",
            i,
            cluster.len(),
            cols,
            need["ExtAlu"],
            need["PrefixSumChecks"],
            need["Select"],
            need["BaseAlu"],
            need["MemoryVar"],
            if fork_chips.is_empty() { String::new() } else { format!("[{}] ", fork_chips.join(",")) },
            if over.is_empty() { String::new() } else { format!("OVER upstream: {}", over.join(" ")) },
        );
        max_fork = Some(match max_fork {
            None => ec,
            Some(a) => max_count(a, ec),
        });
        if fork_chips.is_empty() {
            max_upstream_like = Some(match max_upstream_like {
                None => ec,
                Some(a) => max_count(a, ec),
            });
        }
    }
    println!("({} clusters in {:.0} s)\n", clusters.len(), t_all.elapsed().as_secs_f64());

    // ---- compose fixed point, as compute_compress_shape does ----------------
    let settle = |label: &str, start: SP1RecursionProofShape| -> BTreeMap<String, usize> {
        let mut current = heights_of(&start);
        loop {
            let input = shape_of(&current).dummy_input(
                DEFAULT_ARITY,
                VK_MAP_HEIGHT,
                compress_chips.clone(),
                c_mlrc,
                c_fri,
                c_lsh,
            );
            let program = compose_program_from_input(&rec_compress, true, &input);
            let need = heights_of(&build_shape_from_recursion_air_event_count(&program.event_counts));
            let mut grew = false;
            for (chip, h) in need {
                let cur = current.entry(chip).or_insert(0);
                if h > *cur {
                    *cur = h;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        for (chip, h) in current.iter_mut() {
            if chip == "PublicValues" {
                // The event-count mapping reports 1; the real preprocessed table,
                // which is what upstream's pinned shape records, is 16 rows.
                *h = (*h).max(16);
            } else {
                *h = h.next_multiple_of(32);
            }
        }
        println!("{label:28} {}", fmt_heights(&current));
        current
    };

    println!("== compress shapes ==");
    println!("{:28} {}", "upstream pinned", fmt_heights(&upstream));
    let recomputed_upstream_like = settle(
        "recomputed, no Pasta clusters",
        build_shape_from_recursion_air_event_count(&max_upstream_like.expect("some cluster")),
    );
    let fork_shape = settle(
        "recomputed, all clusters",
        build_shape_from_recursion_air_event_count(&max_fork.expect("some cluster")),
    );
    println!(
        "tool self-check (no-Pasta recompute == upstream pinned): {}\n",
        recomputed_upstream_like == upstream
    );

    // ---- 2. compress shape -> shrink program -> shrink shape -> wrap input ---
    let shrink_verifier = CpuSP1ProverComponents::shrink_verifier();
    let shrink_machine = shrink_verifier.shard_verifier().machine().clone();
    let shrink_chips: BTreeSet<_> = shrink_machine.chips().iter().cloned().collect();
    let s_mlrc = shrink_verifier.max_log_row_count();
    let s_fri = *shrink_verifier.fri_config();
    let s_lsh = shrink_verifier.log_stacking_height() as usize;
    let trace_generator = DefaultTraceGenerator::new(shrink_machine);

    println!("== compress shape -> shrink shape -> wrap input ==");
    println!("compress: log_stacking_height {c_lsh}, max_log_row_count {c_mlrc}");
    println!("shrink:   log_stacking_height {s_lsh}, max_log_row_count {s_mlrc}\n");

    // The real wrap verifying key, computed the way WrapProver::new does
    // (worker/prover/recursion.rs:1165-1190) but without a proof: compile the
    // wrap program from the dummy input and run setup.
    let rec_shrink = recursive_verifier(shrink_verifier.shard_verifier());
    let wrap_prover = CpuWrapProverBuilder.build();
    let upstream_wrap_vk = vk_prefix(WRAP_VK_BYTES);
    let mut vk_by_input: BTreeMap<String, String> = BTreeMap::new();

    let mut wrap_hashes = Vec::new();
    // The shape actually baked into this build, which is what production uses.
    let in_tree: BTreeMap<String, usize> = {
        let raw = std::fs::read_to_string("vendor/sp1/crates/prover/compress_shape.json")
            .context("read the in-tree compress_shape.json")?;
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        serde_json::from_value(v["shape"]["heights"].clone())?
    };
    let short: Vec<String> = fork_shape
        .iter()
        .filter(|(k, v)| in_tree.get(*k).is_none_or(|have| have < v))
        .map(|(k, v)| format!("{k} needs {v}"))
        .collect();
    println!(
        "in-tree compress_shape.json covers every cluster: {}\n",
        if short.is_empty() { "yes".to_string() } else { format!("NO -- {}", short.join(", ")) }
    );

    // The last arm is a negative control: depth 19 is what `pasta-full-clusters`
    // gives, and it must NOT match, or this tool is as vacuous as test_wrap_vk.
    for (label, heights, vk_height) in [
        ("upstream pinned", upstream.clone(), VK_MAP_HEIGHT),
        ("recomputed, all clusters", fork_shape.clone(), VK_MAP_HEIGHT),
        ("in-tree compress_shape.json", in_tree.clone(), VK_MAP_HEIGHT),
        ("CONTROL: in-tree shape at VK-map depth 19", in_tree.clone(), VK_MAP_HEIGHT + 1),
    ] {
        let area = |shape: &BTreeMap<String, usize>, chips: &BTreeSet<_>, pre: bool| -> usize {
            chips
                .iter()
                .map(|c: &sp1_hypercube::Chip<_, _>| {
                    shape[c.name()]
                        * if pre {
                            c.preprocessed_width()
                        } else {
                            c.cost() as usize - c.preprocessed_width()
                        }
                })
                .sum()
        };
        let c_pre = area(&heights, &compress_chips, true);
        let c_main = area(&heights, &compress_chips, false);

        let input =
            shape_of(&heights).dummy_input(1, vk_height, compress_chips.clone(), c_mlrc, c_fri, c_lsh);
        let program = Arc::new(shrink_program_from_input(&rec_compress, true, &input));

        // Exactly what preprocessed_table_heights reports for the shrink pk.
        let traces = trace_generator
            .generate_preprocessed_traces(program, s_mlrc, ProverSemaphore::new(1))
            .await;
        let shrink_heights: BTreeMap<String, usize> = traces
            .preprocessed_traces
            .iter()
            .map(|(name, trace)| (name.to_owned(), trace.num_real_entries()))
            .collect();
        drop(traces);

        let s_pre = area(&shrink_heights, &shrink_chips, true);
        let s_main = area(&shrink_heights, &shrink_chips, false);
        let unit = 1usize << s_lsh;

        let wrap_input = shape_of(&shrink_heights).dummy_input(
            1,
            vk_height,
            shrink_chips.clone(),
            s_mlrc,
            s_fri,
            s_lsh,
        );
        let wrap_sha = sha(&bincode::serialize(&wrap_input).context("serialize wrap input")?);

        println!("{label}");
        println!(
            "  compress multiples   pre {} main {}",
            c_pre.div_ceil(1 << c_lsh),
            c_main.div_ceil(1 << c_lsh)
        );
        println!("  shrink heights       {}", fmt_heights(&shrink_heights));
        println!(
            "  shrink area          pre {s_pre} = {} x 2^{s_lsh} (slack {} cells)   main {s_main} = {} x 2^{s_lsh} (slack {} cells)",
            s_pre.div_ceil(unit),
            s_pre.div_ceil(unit) * unit - s_pre,
            s_main.div_ceil(unit),
            s_main.div_ceil(unit) * unit - s_main,
        );
        println!("  wrap dummy_input     {wrap_sha}");
        let vk_hash = match vk_by_input.get(&wrap_sha) {
            Some(h) => h.clone(),
            None => {
                let t = Instant::now();
                let program = Arc::new(wrap_program_from_input(&rec_shrink, true, &wrap_input));
                let (_, wrap_vk) = wrap_prover.setup(program, ProverSemaphore::new(1)).await;
                let h = vk_prefix(&bincode::serialize(&wrap_vk).context("serialize wrap vk")?);
                println!("  (wrap program compiled and set up in {:.0} s)", t.elapsed().as_secs_f64());
                vk_by_input.insert(wrap_sha.clone(), h.clone());
                h
            }
        };
        println!(
            "  COMPUTED WRAP VK     {vk_hash}   upstream baked {upstream_wrap_vk}   {}\n",
            if vk_hash == upstream_wrap_vk { "MATCH" } else { "MISMATCH" }
        );
        wrap_hashes.push((label, wrap_sha));
    }

    let same = wrap_hashes[0].1 == wrap_hashes[2].1;
    let control_differs = wrap_hashes[3].1 != wrap_hashes[0].1;
    println!(
        "wrap input, upstream pinned vs in-tree compress shape: {}",
        if same { "IDENTICAL" } else { "DIFFERENT" }
    );
    println!(
        "negative control (depth 19) differs from upstream: {}",
        if control_differs { "yes, as it must" } else { "NO -- this tool cannot detect a mismatch" }
    );
    if same && control_differs {
        println!(
            "=> same wrap program and wrap vk as upstream: its released Groth16 artifacts\n\
             apply to this fork, with the in-tree compress shape and VK-map depth {VK_MAP_HEIGHT}."
        );
        Ok(())
    } else {
        anyhow::bail!("the wrap vk invariant does not hold; see the output above")
    }
}
