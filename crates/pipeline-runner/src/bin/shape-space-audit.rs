//! Audit the recursion shape space: the index arithmetic the VK-map plan rests
//! on, checked against the real `create_all_input_shapes` enumeration.
//!
//! Why this exists. An ELF-scoped VK map is built by handing `build_vks` a set
//! of *indices* into `create_all_input_shapes` (`prover/src/shapes.rs:702`).
//! Which indices are the right ones is a claim about that enumeration's
//! structure:
//!
//! 1. the shape is a product of (preprocessed multiple, main multiple, padding
//!    columns, cluster), and the padding dimensions collapse because
//!    `(1 << CORE_LOG_STACKING_HEIGHT).div_ceil(1 << CORE_MAX_LOG_ROW_COUNT)`
//!    is 1 at 21 and 22;
//! 2. the preprocessed multiple is a function of the program, so one ELF
//!    occupies one contiguous block of the space;
//! 3. within a block, the main multiple runs 1..=T-pp for a single constant T,
//!    and the cluster is the fastest-varying coordinate.
//!
//! Decoding a recorded map's indices under (2) and (3) is how we concluded that
//! our guest sits at preprocessed multiple 3 and already spans the whole legal
//! main range. If the arithmetic is wrong, the conclusion is wrong, the map is
//! built over the wrong programs, and the failure surfaces as `vk not allowed`
//! at proving time -- after the money is spent.
//!
//! So this derives nothing by hand: it reads the real enumeration, recovers the
//! block structure from it, and checks the closed form against every entry. The
//! fuzz arm then round-trips random indices and random coordinates through both
//! directions, which is what catches an off-by-one that an exhaustive forward
//! scan can still agree with.
//!
//! Exits non-zero on any failure. Pure arithmetic over the shape list: no
//! proving, no GPU, seconds.
use std::collections::BTreeSet;

use clap::Parser;
use sp1_core_machine::riscv::RiscvAir;
use sp1_primitives::SP1Field;
use sp1_prover::shapes::{
    create_all_input_shapes, normalize_program_parameter_space, SP1RecursionProgramShape,
};

/// `prover/src/shapes.rs:71`.
const DEFAULT_ARITY: usize = 4;
/// One Compose per arity, then Deferred and Shrink (`shapes.rs:730-738`).
const FIXED_TAIL: usize = DEFAULT_ARITY + 2;
/// `prover/src/components.rs:16`.
const CORE_LOG_STACKING_HEIGHT: u32 = 21;
/// What upstream's baked map implies, and what the wrap circuit expects.
const EXPECTED_DEPTH: u32 = 18;

#[derive(Parser)]
struct Args {
    /// Random index/coordinate round-trips to run.
    #[arg(long, default_value = "200000")]
    fuzz: u64,
    /// Seed for the fuzz arm. The generator is deterministic, so a failing run
    /// reproduces exactly.
    #[arg(long, default_value = "0")]
    seed: u64,
    /// Report the block layout for this preprocessed multiple. 3 is our guest's.
    #[arg(long, default_value = "3")]
    bucket: usize,
}

/// SplitMix64. Inline rather than a `rand` dependency, and seeded so a failure
/// is reproducible from the printed seed.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A Normalize shape's coordinates, as this audit models them.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Coord {
    preprocessed_multiple: usize,
    main_multiple: usize,
    cluster: usize,
}

fn fail(msg: String) -> ! {
    eprintln!("\nFAIL: {msg}");
    std::process::exit(1);
}

fn main() {
    let args = Args::parse();
    let machine = RiscvAir::<SP1Field>::machine();
    let shape = machine.shape();
    let clusters = &shape.chip_clusters;
    let num_clusters = clusters.len();
    let shapes = create_all_input_shapes(shape, DEFAULT_ARITY);
    let (max_pp, max_main_reported, capacity) = normalize_program_parameter_space();

    println!("clusters                 {num_clusters}");
    println!("shape space              {} entries", shapes.len());
    println!("max preprocessed mult.   {max_pp}");
    println!("max main multiple        {max_main_reported}");
    println!("reported capacity        {capacity}  (shapes.rs counts 0..=max_pp; the loop is 1..=max_pp)");

    // ---- 1. The tail is exactly the fixed programs. -------------------------
    let tail = &shapes[shapes.len() - FIXED_TAIL..];
    let mut expected_tail: Vec<SP1RecursionProgramShape> =
        (1..=DEFAULT_ARITY).map(SP1RecursionProgramShape::Compose).collect();
    expected_tail.push(SP1RecursionProgramShape::Deferred);
    expected_tail.push(SP1RecursionProgramShape::Shrink);
    if tail != expected_tail.as_slice() {
        fail(format!("the last {FIXED_TAIL} entries are not Compose(1..={DEFAULT_ARITY}), Deferred, Shrink: {tail:?}"));
    }
    // And the entry just before them is a Normalize shape, which is what makes a
    // wider tail silently wrong rather than loud.
    match &shapes[shapes.len() - FIXED_TAIL - 1] {
        SP1RecursionProgramShape::Normalize(_) => {}
        other => fail(format!("expected a Normalize shape before the fixed tail, got {other:?}")),
    }
    println!("\n[1] fixed tail           OK  last {FIXED_TAIL} are Compose(1..={DEFAULT_ARITY}), Deferred, Shrink");
    println!("                             (a 12-wide tail would take 6 Normalize shapes with it)");

    // ---- 2. Recover the coordinates of every Normalize entry. ---------------
    let normalize_len = shapes.len() - FIXED_TAIL;
    let mut coords: Vec<Coord> = Vec::with_capacity(normalize_len);
    for (i, s) in shapes[..normalize_len].iter().enumerate() {
        let SP1RecursionProgramShape::Normalize(core) = s else {
            fail(format!("entry {i} is not a Normalize shape: {s:?}"));
        };
        let cluster = match clusters.iter().position(|c| c == &core.shard_chips) {
            Some(c) => c,
            None => fail(format!("entry {i} has a chip set that is not one of the {num_clusters} clusters")),
        };
        let pp = core.preprocessed_area >> CORE_LOG_STACKING_HEIGHT;
        let main = core.main_area >> CORE_LOG_STACKING_HEIGHT;
        if pp << CORE_LOG_STACKING_HEIGHT != core.preprocessed_area
            || main << CORE_LOG_STACKING_HEIGHT != core.main_area
        {
            fail(format!("entry {i} has an area that is not a multiple of 2^{CORE_LOG_STACKING_HEIGHT}"));
        }
        coords.push(Coord { preprocessed_multiple: pp, main_multiple: main, cluster });
    }

    // ---- 3. The enumeration is lexicographic, cluster fastest. --------------
    // Blocks: one per preprocessed multiple, contiguous, each main_multiple
    // repeated once per cluster in cluster order.
    let mut block_start: Vec<usize> = Vec::new();
    let mut block_mains: Vec<usize> = Vec::new();
    let mut i = 0usize;
    let mut expected_pp = 1usize;
    while i < normalize_len {
        block_start.push(i);
        let pp = coords[i].preprocessed_multiple;
        if pp != expected_pp {
            fail(format!("block {} starts at preprocessed multiple {pp}, expected {expected_pp}", block_start.len()));
        }
        let mut mains = 0usize;
        while i < normalize_len && coords[i].preprocessed_multiple == pp {
            let main = coords[i].main_multiple;
            if main != mains + 1 {
                fail(format!("at {i}: main multiple {main}, expected {}", mains + 1));
            }
            for c in 0..num_clusters {
                let got = coords[i + c];
                if got != (Coord { preprocessed_multiple: pp, main_multiple: main, cluster: c }) {
                    fail(format!("at {}: expected cluster {c} of pp {pp} main {main}, got {got:?}", i + c));
                }
            }
            i += num_clusters;
            mains += 1;
        }
        block_mains.push(mains);
        expected_pp += 1;
    }
    let num_blocks = block_start.len();
    if num_blocks != max_pp {
        fail(format!("{num_blocks} blocks but normalize_program_parameter_space says max_pp = {max_pp}"));
    }
    println!("[2] enumeration order    OK  {num_blocks} contiguous blocks, cluster fastest, main next");

    // ---- 4. The block sizes follow a single constant T: main runs 1..=T-pp. --
    let t = block_mains[0] + 1;
    for (b, mains) in block_mains.iter().enumerate() {
        let pp = b + 1;
        if mains + pp != t {
            fail(format!("block pp={pp} has {mains} main multiples; T is not constant ({} vs {t})", mains + pp));
        }
    }
    let per_cluster: usize = block_mains.iter().sum();
    println!("[3] block sizes          OK  main multiple runs 1..={t}-pp for T = {t}");
    println!("                             {per_cluster} shapes per cluster, {normalize_len} Normalize entries");

    // ---- 5. The closed form, checked against every entry. -------------------
    // offset(pp) = num_clusters * sum_{q<pp} (T - q)
    let offset = |pp: usize| -> usize {
        (1..pp).map(|q| t - q).sum::<usize>() * num_clusters
    };
    let encode = |c: Coord| -> usize {
        offset(c.preprocessed_multiple) + (c.main_multiple - 1) * num_clusters + c.cluster
    };
    for (i, c) in coords.iter().enumerate() {
        if encode(*c) != i {
            fail(format!("closed form puts {c:?} at {} but it is at {i}", encode(*c)));
        }
    }
    println!("[4] closed form          OK  checked against all {normalize_len} entries");

    // ---- 6. Fuzz: round-trip random indices and random coordinates. ---------
    let decode = |i: usize| -> Coord {
        let mut pp = 1usize;
        let mut rest = i;
        loop {
            let block = (t - pp) * num_clusters;
            if rest < block {
                break;
            }
            rest -= block;
            pp += 1;
        }
        Coord {
            preprocessed_multiple: pp,
            main_multiple: rest / num_clusters + 1,
            cluster: rest % num_clusters,
        }
    };
    let mut rng = Rng(args.seed);
    for _ in 0..args.fuzz {
        let i = rng.below(normalize_len);
        let c = decode(i);
        if c != coords[i] {
            fail(format!("decode({i}) = {c:?} but the enumeration has {:?}", coords[i]));
        }
        if encode(c) != i {
            fail(format!("encode(decode({i})) = {}", encode(c)));
        }
        // And from the other direction: a random legal coordinate.
        let pp = rng.below(max_pp) + 1;
        let c = Coord {
            preprocessed_multiple: pp,
            main_multiple: rng.below(t - pp) + 1,
            cluster: rng.below(num_clusters),
        };
        let j = encode(c);
        if j >= normalize_len {
            fail(format!("encode({c:?}) = {j} is past the {normalize_len} Normalize entries"));
        }
        if decode(j) != c || coords[j] != c {
            fail(format!("round trip broke for {c:?} at index {j}"));
        }
    }
    println!("[5] fuzz                 OK  {} round trips, seed {}", args.fuzz, args.seed);

    // ---- 7. The depth the whole ceremony-free result rests on. --------------
    let keys = shapes.iter().cloned().collect::<BTreeSet<_>>().len();
    let depth = keys.next_power_of_two().trailing_zeros();
    let duplicates = shapes.len() - keys;
    if depth != EXPECTED_DEPTH {
        fail(format!(
            "deduplicated key count {keys} gives depth {depth}, not upstream's {EXPECTED_DEPTH}"
        ));
    }
    // The chunk worker builds each vk at log2_ceil(deduplicated), while
    // `allowed_vk_height` pads to the full list. Both must land on 18 or the
    // prover and the verifier disagree about the tree.
    let padded_depth = shapes.len().next_power_of_two().trailing_zeros();
    if padded_depth != depth {
        fail(format!(
            "the deduplicated count gives depth {depth} but the padded list gives {padded_depth}"
        ));
    }
    println!("[6] vk tree depth        OK  {keys} distinct ({duplicates} duplicates) -> depth {depth}");

    // ---- 8. What an ELF-scoped map over one block would cost. ---------------
    if args.bucket >= 1 && args.bucket <= max_pp {
        let b = args.bucket;
        let size = block_mains[b - 1] * num_clusters;
        println!("\nblock pp={b}: indices {}..{} ({size} shapes)", offset(b), offset(b) + size);
        println!("  main multiples        1..={}", block_mains[b - 1]);
        println!("  all-cluster map       {size} programs + {FIXED_TAIL} fixed = {}", size + FIXED_TAIL);
        println!("  at 0.285 s/program    {:.0} min", (size + FIXED_TAIL) as f64 * 0.285 / 60.0);
    }

    println!("\nOK: the shape-space arithmetic holds.");
}
