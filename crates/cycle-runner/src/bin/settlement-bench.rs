//! Local CPU execution + shard/cell accounting + fixture export for the combined
//! `o1-verifier-settlement` guest (Pickles mainnet SNARK + dual-root Mina→EVM mirror).
use anyhow::{anyhow, ensure, Context, Result};
use clap::Parser;
use pickles_verifier::{
    mirror::{
        dedup_account_updates, empty_keccak_subtrees, fp_from_le_bytes, fp_to_le_bytes,
        keccak_bound_leaf, keccak_node, verify_and_apply_settlement, AccountUpdateWitness,
        EvmAccountFields, MinaLedgerHasher, SettlementPublicValues, SettlementWitness,
        MINA_LEDGER_DEPTH,
    },
    types::StepField,
    wire::{parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof},
};
use sha2::{Digest, Sha256};
use sp1_core_executor::{Program, RetainedEventsPreset, SP1CoreOpts, ShardingThreshold};
use sp1_core_machine::{
    io::SP1Stdin,
    riscv::RiscvAir,
    utils::generate_records_streaming_with_summary,
};
use sp1_hypercube::air::MachineAir;
use sp1_primitives::SP1Field;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    elf: PathBuf,
    #[arg(long, default_value = "fixtures/mainnet-blockchain-snark")]
    fixture_dir: PathBuf,
    #[arg(long, default_value_t = 64)]
    accounts: u32,
    #[arg(long, default_value_t = MINA_LEDGER_DEPTH)]
    depth: u32,
    #[arg(long)]
    export_dir: Option<PathBuf>,
}

/// Build a cryptographically consistent sparse depth-`depth` Mina Poseidon + Keccak
/// `SettlementWitness` with `accounts` raw updates (25% hot-account repeats).
fn build_consistent_witness(accounts: u32, depth: u32) -> SettlementWitness {
    let hasher = MinaLedgerHasher::new(depth);
    let p_zeros = hasher.empty_subtree_hashes();
    let k_zeros = empty_keccak_subtrees(depth);

    let make_account = |seed: u64| -> (Vec<[u8; 32]>, EvmAccountFields) {
        let fields = vec![
            fp_to_le_bytes(StepField::from(seed + 1)),
            fp_to_le_bytes(StepField::from(seed.wrapping_mul(7) + 3)),
            fp_to_le_bytes(StepField::from(seed.wrapping_mul(13) + 5)),
            fp_to_le_bytes(StepField::from(seed.wrapping_mul(29) + 11)),
        ];
        let evm = EvmAccountFields {
            public_key_x: fp_to_le_bytes(StepField::from(seed + 1)),
            balance: 1_000_000_000 + seed * 1_000,
            nonce: (seed & 0xFFFF) as u32,
            flags: (seed & 1) as u32,
            zkapp_state_hash: fp_to_le_bytes(StepField::from(seed.wrapping_mul(29) + 11)),
        };
        (fields, evm)
    };

    let mask = if depth >= 64 { u64::MAX } else { (1u64 << depth) - 1 };
    let unique_pool = ((accounts as u64 * 3) / 4).max(1);

    // Populate initial sparse tree with the unique touched accounts at their old states.
    let mut p_nodes: Vec<HashMap<u64, StepField>> = vec![HashMap::new(); depth as usize + 1];
    let mut k_nodes: Vec<HashMap<u64, [u8; 32]>> = vec![HashMap::new(); depth as usize + 1];

    let mut updates = Vec::with_capacity(accounts as usize);
    let mut cur_seed_by_idx: HashMap<u64, u64> = HashMap::new();

    for seq in 0..accounts {
        let logical = (seq as u64) % unique_pool;
        let leaf_idx = logical.wrapping_mul(0x9E37_79B9_7F4A_7C15) & mask;
        let old_seed = *cur_seed_by_idx.entry(leaf_idx).or_insert(logical);
        let new_seed = (seq as u64) + 1_000_000;
        cur_seed_by_idx.insert(leaf_idx, new_seed);

        let (old_f, old_e) = make_account(old_seed);
        let (new_f, new_e) = make_account(new_seed);

        if seq < unique_pool as u32 {
            let old_fps: Vec<StepField> = old_f.iter().map(fp_from_le_bytes).collect();
            let old_hp = hasher.hash_account_fields(&old_fps);
            p_nodes[0].insert(leaf_idx, old_hp);
            k_nodes[0].insert(leaf_idx, keccak_bound_leaf(&fp_to_le_bytes(old_hp), &old_e));
        }

        updates.push(AccountUpdateWitness {
            leaf_idx,
            seq,
            old_account_fields: old_f,
            new_account_fields: new_f,
            old_evm_fields: old_e,
            new_evm_fields: new_e,
        });
    }

    // Build sparse old Poseidon & Keccak trees bottom-up.
    for h in 0..depth as usize {
        let mut parents: Vec<u64> = p_nodes[h].keys().map(|idx| idx >> 1).collect();
        parents.sort_unstable();
        parents.dedup();
        for p in parents {
            let l_idx = p << 1;
            let r_idx = l_idx | 1;
            let pl = p_nodes[h].get(&l_idx).copied().unwrap_or(p_zeros[h]);
            let pr = p_nodes[h].get(&r_idx).copied().unwrap_or(p_zeros[h]);
            p_nodes[h + 1].insert(p, hasher.hash_node(h as u32, pl, pr));

            let kl = k_nodes[h].get(&l_idx).copied().unwrap_or(k_zeros[h]);
            let kr = k_nodes[h].get(&r_idx).copied().unwrap_or(k_zeros[h]);
            k_nodes[h + 1].insert(p, keccak_node(&kl, &kr));
        }
    }

    let old_poseidon_root = fp_to_le_bytes(
        p_nodes[depth as usize].get(&0).copied().unwrap_or(p_zeros[depth as usize]),
    );
    let old_keccak_global_root =
        k_nodes[depth as usize].get(&0).copied().unwrap_or(k_zeros[depth as usize]);

    // Extract external siblings in sorted bottom-up multiproof order.
    let unique = dedup_account_updates(&updates);
    let mut poseidon_siblings = Vec::new();
    let mut keccak_siblings = Vec::new();
    let mut frontier: Vec<u64> = unique.iter().map(|u| u.leaf_idx).collect();
    for h in 0..depth as usize {
        let mut next = Vec::new();
        let mut i = 0;
        while i < frontier.len() {
            let idx = frontier[i];
            if (idx & 1 == 0) && (i + 1 < frontier.len()) && (frontier[i + 1] == idx + 1) {
                next.push(idx >> 1);
                i += 2;
            } else {
                let sib_idx = idx ^ 1;
                let ps = p_nodes[h].get(&sib_idx).copied().unwrap_or(p_zeros[h]);
                let ks = k_nodes[h].get(&sib_idx).copied().unwrap_or(k_zeros[h]);
                poseidon_siblings.push(fp_to_le_bytes(ps));
                keccak_siblings.push(ks);
                next.push(idx >> 1);
                i += 1;
            }
        }
        frontier = next;
    }

    SettlementWitness {
        depth,
        block_height: 412_000,
        protocol_state_preimage: None,
        old_poseidon_root,
        old_keccak_global_root,
        updates,
        poseidon_siblings,
        keccak_siblings,
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let elf = fs::read(&args.elf).context("read settlement guest ELF")?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow!("{e}"))?);

    // Load real Mina mainnet Pickles proof fixture.
    let wrap_proof =
        parse_wrap_proof(&fs::read_to_string(args.fixture_dir.join("proof.serde.json"))?)
            .map_err(|e| anyhow!("parse proof: {e}"))?;
    let ocaml_proof = OcamlProof::parse(&fs::read_to_string(
        args.fixture_dir.join("public_input_skeleton.json"),
    )?)
    .map_err(|e| anyhow!("parse public_input_skeleton: {e}"))?;
    let app_state = parse_app_statement_fields(&fs::read_to_string(
        args.fixture_dir.join("app_statement.json"),
    )?)
    .map_err(|e| anyhow!("parse app_statement: {e}"))?;
    let wrap_vk = parse_wrap_vk(&fs::read_to_string(args.fixture_dir.join("vk.serde.json"))?)
        .map_err(|e| anyhow!("parse vk: {e}"))?;
    let proof = ocaml_proof
        .into_verifiable(wrap_proof, &wrap_vk, &app_state)
        .map_err(|e| anyhow!("into_verifiable: {e}"))?;

    let witness = build_consistent_witness(args.accounts, args.depth);
    let expected_pv: SettlementPublicValues =
        verify_and_apply_settlement(proof.app_state[0], &witness);
    let expected_pv_bytes = expected_pv.to_abi_bytes();

    if let Some(dir) = &args.export_dir {
        fs::create_dir_all(dir)?;
        let proof_bytes = bincode::serialize(&proof)?;
        let witness_bytes = bincode::serialize(&witness)?;
        fs::write(dir.join("proof-input.bin"), &proof_bytes)?;
        fs::write(dir.join("receipt-input.bin"), &witness_bytes)?;
        fs::write(dir.join("public-values.bin"), &expected_pv_bytes)?;
        println!(
            "exported inputs to {}: proof={}B witness={}B pv={}B pv_sha256={}",
            dir.display(),
            proof_bytes.len(),
            witness_bytes.len(),
            expected_pv_bytes.len(),
            hex::encode(Sha256::digest(&expected_pv_bytes))
        );
    }

    let mut stdin = SP1Stdin::new();
    stdin.write(&proof);
    stdin.write(&witness);

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
    opts.retained_events_presets = HashSet::from([
        RetainedEventsPreset::Bls12381Field,
        RetainedEventsPreset::Bn254Field,
        RetainedEventsPreset::PallasAdd,
        RetainedEventsPreset::Poseidon2,
        RetainedEventsPreset::Sha256,
        RetainedEventsPreset::U256Ops,
        RetainedEventsPreset::Uint256MulMod,
        RetainedEventsPreset::VestaAdd,
    ]);

    let chips = RiscvAir::<SP1Field>::chips();
    let started = Instant::now();
    let mut records = 0u64;
    let mut total_main_cells = 0u64;
    let mut chip_cells: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new(); // (shards, rows, cells)

    let summary = generate_records_streaming_with_summary::<SP1Field, _>(
        program,
        stdin,
        opts,
        [0; 4],
        |record| {
            records += 1;
            for chip in chips.iter() {
                if !chip.included(&record) {
                    continue;
                }
                let rows = chip.num_rows(&record).unwrap_or(0) as u64;
                let width = chip.cost().saturating_sub(chip.preprocessed_width() as u64);
                let cells = rows * width;
                total_main_cells += cells;
                let e = chip_cells.entry(chip.name().to_owned()).or_default();
                e.0 += 1;
                e.1 += rows;
                e.2 += cells;
            }
            true
        },
    )?;

    let got_pv = summary.public_values.context("no public values committed")?;
    ensure!(
        got_pv.as_slice() == expected_pv_bytes.as_slice(),
        "public values mismatch! got len={}, expected len={}",
        got_pv.len(),
        expected_pv_bytes.len()
    );

    println!(
        "SETTLEMENT_OK in {:.1}s: accounts={} depth={} cycles={} shards={} total_main_cells={} pv_len={} pv_sha256={}",
        started.elapsed().as_secs_f64(),
        args.accounts,
        args.depth,
        summary.cycles,
        records,
        total_main_cells,
        got_pv.len(),
        hex::encode(Sha256::digest(&got_pv)),
    );
    for key in ["Add", "Global", "KeccakPermute", "PallasPoseidonPermute", "VestaAddAssign", "PallasAddAssign", "SyscallPrecompile"] {
        if let Some((s, r, c)) = chip_cells.get(key) {
            println!("  {:24} shards={:3}  rows={:10}  cells={:13}", key, s, r, c);
        }
    }
    Ok(())
}
