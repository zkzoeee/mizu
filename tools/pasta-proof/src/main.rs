#![recursion_limit = "512"]
//! Real, CPU-only SP1 core proofs for four constrained Pasta operations:
//! Vesta add, Vesta double, Pallas add and Pallas double.
//!
//! These are synthetic SP1 `Program` fixtures, not compiled guest ELFs (the
//! compiled guest is a separate crate and a separate artifact row). Negative
//! cases construct fresh proofs from malicious witnesses and require the real
//! cryptographic verifier to reject; a prover panic is reported separately and
//! never counted as a verifier rejection.

use anyhow::{bail, ensure, Context, Result};
use num::{BigUint, One, Zero};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use slop_algebra::{AbstractField, PrimeField32};
use slop_basefold::FriConfig;
use slop_challenger::IopCtx;
use slop_multilinear::{Mle, PaddedMle};
use sp1_core_executor::{
    add_halt,
    events::{EllipticCurveAddEvent, EllipticCurveDoubleEvent, PrecompileEvent},
    rv64im_costs, ExecutionRecord, Instruction, Opcode, Program, RiscvAirId, SP1CoreOpts,
    SyscallCode,
};
use sp1_core_machine::{
    io::SP1Stdin,
    riscv::RiscvAir,
    syscall::precompiles::weierstrass::{
        num_weierstrass_add_cols_supervisor, num_weierstrass_double_cols_supervisor,
        WeierstrassAddAssignChip, WeierstrassAddAssignCols, WeierstrassDoubleAssignChip,
        WeierstrassDoubleAssignCols,
    },
    utils::generate_records,
};
use sp1_curves::{
    params::{FieldParameters, NumWords},
    weierstrass::pasta::{Pallas, PallasBaseField, Vesta, VestaBaseField},
};
use sp1_hypercube::{
    air::{MachineAir, PublicValues},
    debug_constraints,
    prover::{
        CpuShardProver, MainTraceData, ProverSemaphore, ProvingKey, SP1InnerPcsProver, ShardData,
        SimpleProver, TraceGenerator,
    },
    Chip, InnerSC, MachineProof, MachineVerifyingKey, SP1InnerPcs, SP1PcsProofInner, ShardVerifier,
};
use sp1_primitives::{SP1Field, SP1GlobalContext};
use std::{
    borrow::{Borrow, BorrowMut},
    collections::BTreeMap,
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
use strum::IntoEnumIterator;

#[cfg(feature = "cuda")]
mod gpu;
mod pipeline;

type Air = RiscvAir<SP1Field>;
type Cpu = CpuShardProver<SP1GlobalContext, SP1InnerPcs, SP1InnerPcsProver, Air>;
type Key = ProvingKey<SP1GlobalContext, InnerSC<Air>, Cpu>;
type Proof = MachineProof<SP1GlobalContext, SP1PcsProofInner>;
type Point = (BigUint, BigUint);
type TraceData = MainTraceData<SP1Field, Air, slop_alloc::CpuBackend>;

const LOG_STACKING_HEIGHT: u32 = 17;
const MAX_LOG_ROW_COUNT: usize = 17; // The fixed Range table needs 2^17 rows.
const FIXTURE_BASE: u64 = 0x10000; // SP1 requires aligned data in [2^16, 2^48).
const CALL_STRIDE: u64 = 0x1000; // One page per call keeps every ADDI offset < 0x800.
const P_OFFSET: u64 = 0x000;
const Q_OFFSET: u64 = 0x040;
const EXPECTED_OFFSET: u64 = 0x080;

// ---------------------------------------------------------------------------
// Operation table. Syscall encodings are the pre-existing experimental ones.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Curve {
    Vesta,
    Pallas,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Add,
    Double,
}

#[derive(Clone, Copy)]
struct Op {
    id: &'static str,
    curve: Curve,
    kind: Kind,
    syscall: SyscallCode,
    chip: &'static str,
    air: RiscvAirId,
}

const OPS: [Op; 4] = [
    Op {
        id: "vesta-add",
        curve: Curve::Vesta,
        kind: Kind::Add,
        syscall: SyscallCode::VESTA_ADD,
        chip: "VestaAddAssign",
        air: RiscvAirId::VestaAddAssign,
    },
    Op {
        id: "vesta-double",
        curve: Curve::Vesta,
        kind: Kind::Double,
        syscall: SyscallCode::VESTA_DOUBLE,
        chip: "VestaDoubleAssign",
        air: RiscvAirId::VestaDoubleAssign,
    },
    Op {
        id: "pallas-add",
        curve: Curve::Pallas,
        kind: Kind::Add,
        syscall: SyscallCode::PALLAS_ADD,
        chip: "PallasAddAssign",
        air: RiscvAirId::PallasAddAssign,
    },
    Op {
        id: "pallas-double",
        curve: Curve::Pallas,
        kind: Kind::Double,
        syscall: SyscallCode::PALLAS_DOUBLE,
        chip: "PallasDoubleAssign",
        air: RiscvAirId::PallasDoubleAssign,
    },
];

fn modulus(curve: Curve) -> BigUint {
    match curve {
        Curve::Vesta => VestaBaseField::modulus(),
        Curve::Pallas => PallasBaseField::modulus(),
    }
}

fn op_width(op: &Op) -> usize {
    match (op.curve, op.kind) {
        (Curve::Vesta, Kind::Add) => num_weierstrass_add_cols_supervisor::<VestaBaseField>(),
        (Curve::Vesta, Kind::Double) => num_weierstrass_double_cols_supervisor::<VestaBaseField>(),
        (Curve::Pallas, Kind::Add) => num_weierstrass_add_cols_supervisor::<PallasBaseField>(),
        (Curve::Pallas, Kind::Double) => {
            num_weierstrass_double_cols_supervisor::<PallasBaseField>()
        }
    }
}

// ---------------------------------------------------------------------------
// Independent reference arithmetic (Fermat inversion), separate from both the
// executor's `sw_add`/`sw_double` and the AIR's witness generator.
// ---------------------------------------------------------------------------

fn sub_mod(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
    (a + m - b) % m
}

fn inv_mod(a: &BigUint, m: &BigUint) -> BigUint {
    assert!(!a.is_zero(), "no inverse of zero");
    let inverse = a.modpow(&(m - BigUint::from(2u32)), m);
    assert_eq!(
        (a * &inverse) % m,
        BigUint::one(),
        "Fermat inversion failed"
    );
    inverse
}

fn on_curve(p: &Point, m: &BigUint) -> bool {
    p.0 < *m && p.1 < *m && (&p.1 * &p.1) % m == ((&p.0 * &p.0 * &p.0) + BigUint::from(5u32)) % m
}

/// Raw incomplete affine addition: requires distinct x coordinates.
fn ref_add(p: &Point, q: &Point, m: &BigUint) -> Point {
    assert!(p.0 != q.0, "ref_add requires distinct x coordinates");
    let slope = (sub_mod(&q.1, &p.1, m) * inv_mod(&sub_mod(&q.0, &p.0, m), m)) % m;
    let x = sub_mod(&sub_mod(&((&slope * &slope) % m), &p.0, m), &q.0, m);
    let y = sub_mod(&((&slope * sub_mod(&p.0, &x, m)) % m), &p.1, m);
    (x, y)
}

/// Raw affine doubling: requires nonzero 2y.
fn ref_double(p: &Point, m: &BigUint) -> Point {
    let two_y = (BigUint::from(2u32) * &p.1) % m;
    assert!(!two_y.is_zero(), "ref_double requires nonzero 2y");
    let slope = ((BigUint::from(3u32) * &p.0 * &p.0) % m * inv_mod(&two_y, m)) % m;
    let x = sub_mod(&sub_mod(&((&slope * &slope) % m), &p.0, m), &p.0, m);
    let y = sub_mod(&((&slope * sub_mod(&p.0, &x, m)) % m), &p.1, m);
    (x, y)
}

/// Complete group law, used only to generate deterministic distinct test points
/// and to check the mixed-operation scalar multiple.
#[derive(Clone, PartialEq, Eq, Debug)]
enum GroupPoint {
    Infinity,
    Finite(BigUint, BigUint),
}

fn group_add(a: &GroupPoint, b: &GroupPoint, m: &BigUint) -> GroupPoint {
    match (a, b) {
        (GroupPoint::Infinity, other) | (other, GroupPoint::Infinity) => other.clone(),
        (GroupPoint::Finite(ax, ay), GroupPoint::Finite(bx, by)) => {
            if ax == bx {
                if ay == by && !ay.is_zero() {
                    let (x, y) = ref_double(&(ax.clone(), ay.clone()), m);
                    GroupPoint::Finite(x, y)
                } else {
                    GroupPoint::Infinity
                }
            } else {
                let (x, y) = ref_add(&(ax.clone(), ay.clone()), &(bx.clone(), by.clone()), m);
                GroupPoint::Finite(x, y)
            }
        }
    }
}

fn generator(curve: Curve) -> Point {
    use sp1_curves::weierstrass::WeierstrassParameters;
    match curve {
        Curve::Vesta => {
            <sp1_curves::weierstrass::pasta::VestaParameters as WeierstrassParameters>::generator()
        }
        Curve::Pallas => {
            <sp1_curves::weierstrass::pasta::PallasParameters as WeierstrassParameters>::generator()
        }
    }
}

/// Deterministic multiples `G, 2G, ..., nG` of the curve generator.
fn multiples(curve: Curve, n: usize) -> Vec<Point> {
    let m = modulus(curve);
    let g = generator(curve);
    let g = GroupPoint::Finite(g.0, g.1);
    let mut out = Vec::with_capacity(n);
    let mut acc = GroupPoint::Infinity;
    for _ in 0..n {
        acc = group_add(&acc, &g, &m);
        match &acc {
            GroupPoint::Finite(x, y) => out.push((x.clone(), y.clone())),
            GroupPoint::Infinity => panic!("generator has unexpectedly small order"),
        }
    }
    out
}

fn words(point: &Point) -> [u64; 8] {
    let mut result = [0u64; 8];
    for (offset, value) in [(0usize, &point.0), (4usize, &point.1)] {
        let limbs = value.to_u64_digits();
        assert!(limbs.len() <= 4, "coordinate exceeds four 64-bit words");
        result[offset..offset + limbs.len()].copy_from_slice(&limbs);
    }
    result
}

fn point_from_words(w: &[u64]) -> Point {
    let build = |s: &[u64]| {
        let mut v = BigUint::zero();
        for (i, word) in s.iter().enumerate() {
            v += BigUint::from(*word) << (64 * i);
        }
        v
    };
    (build(&w[0..4]), build(&w[4..8]))
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_json(path: impl AsRef<Path>, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?).context("write JSON artifact")
}

// ---------------------------------------------------------------------------
// Fixture programs.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Call {
    op: Op,
    p: Point,
    q: Option<Point>,
    expected: Point,
}

fn make_call(op: Op, p: Point, q: Option<Point>) -> Result<Call> {
    let m = modulus(op.curve);
    ensure!(
        on_curve(&p, &m),
        "fixture p is not a canonical point on the curve"
    );
    let expected = match op.kind {
        Kind::Add => {
            let q = q.clone().context("add needs a second point")?;
            ensure!(
                on_curve(&q, &m),
                "fixture q is not a canonical point on the curve"
            );
            ensure!(p.0 != q.0, "raw add precondition: distinct x coordinates");
            ref_add(&p, &q, &m)
        }
        Kind::Double => {
            ensure!(
                !((BigUint::from(2u32) * &p.1) % &m).is_zero(),
                "raw double precondition: nonzero 2y"
            );
            ref_double(&p, &m)
        }
    };
    ensure!(on_curve(&expected, &m), "reference result left the curve");
    Ok(Call { op, p, q, expected })
}

fn load_address(instructions: &mut Vec<Instruction>, register: u8, address: u64) {
    let high = address & !0xfff;
    let low = address & 0xfff;
    assert!(low < 0x800, "ADDI immediate must stay positive");
    instructions.push(Instruction::new(
        Opcode::LUI,
        register,
        high,
        high,
        true,
        true,
    ));
    if low != 0 {
        instructions.push(Instruction::new(
            Opcode::ADDI,
            register,
            register as u64,
            low,
            false,
            true,
        ));
    }
}

/// Public-value words committed by the synthetic fixtures.
///
/// The complete verifier requires that COMMIT and COMMIT_DEFERRED_PROOFS both
/// ran, and it chains their digests across shards. `add_halt` alone produces an
/// execution that cannot pass it. The low twelve bits are kept below 0x800 so
/// each word loads with one LUI plus a positive ADDI immediate.
fn fixture_digest_words(tag: u32) -> [u32; 8] {
    core::array::from_fn(|i| ((0x50u32 + tag) << 24) | (0x41 << 16) | 0x0300 | i as u32)
}

fn fixture_digest_bytes(tag: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, word) in fixture_digest_words(tag).into_iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// Emit the COMMIT and COMMIT_DEFERRED_PROOFS syscalls a complete execution
/// needs, so the synthetic fixtures are verifiable by the complete verifier and
/// not only shard by shard.
fn add_commitments(instructions: &mut Vec<Instruction>, tag: u32) {
    for (index, word) in fixture_digest_words(tag).into_iter().enumerate() {
        load_address(instructions, 5, SyscallCode::COMMIT as u32 as u64);
        load_address(instructions, 10, index as u64);
        load_address(instructions, 11, word as u64);
        instructions.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    }
    for index in 0..sp1_hypercube::air::POSEIDON_NUM_WORDS {
        load_address(
            instructions,
            5,
            SyscallCode::COMMIT_DEFERRED_PROOFS as u32 as u64,
        );
        load_address(instructions, 10, index as u64);
        load_address(instructions, 11, 0);
        instructions.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    }
}

/// Build a synthetic program executing `calls` in order. Inputs and expected
/// outputs live in the program memory image, so the verification key binds
/// them; the program compares all eight output words before a successful HALT.
fn build_program(calls: &[Call]) -> Result<(Arc<Program>, Value)> {
    build_program_at(calls, FIXTURE_BASE, true, false, 0, Some(0), 0, 0)
}

/// `base` places the per-call data pages; `compare` emits the eight LD/BNE
/// output checks. Volume sweeps use a higher base (many calls need a longer
/// instruction region) and can omit the comparisons to isolate the precompile
/// rows from the core rows those comparisons add.
fn build_program_at(
    calls: &[Call],
    base_address: u64,
    compare: bool,
    reuse: bool,
    pad_alu: usize,
    commit_tag: Option<u32>,
    double_a1: u64,
    halt_exit_code: u64,
) -> Result<(Arc<Program>, Value)> {
    ensure!(!calls.is_empty(), "need at least one call");
    ensure!(
        !(reuse && compare),
        "reused slots accumulate in place, so the per-call expected constants do not apply"
    );
    let mut instructions = Vec::new();
    let mut branches = Vec::new();
    let mut image_entries: Vec<(u64, [u64; 8])> = Vec::new();
    let mut call_json = Vec::new();

    for (index, call) in calls.iter().enumerate() {
        // With `reuse`, every call operates on the same two slots, so the
        // precompile accumulates in place the way MSM bucket accumulation does
        // and the memory working set does not grow with the call count.
        let slot = if reuse { 0 } else { index as u64 };
        let base = base_address + slot * CALL_STRIDE;
        let (p_addr, q_addr, expected_addr) =
            (base + P_OFFSET, base + Q_OFFSET, base + EXPECTED_OFFSET);
        for address in [p_addr, q_addr, expected_addr] {
            for limb in 0..8u64 {
                let a = address + 8 * limb;
                ensure!(
                    (1u64 << 16..1u64 << 48).contains(&a) && a % 8 == 0,
                    "fixture address must be 8-byte aligned and in SP1's [2^16, 2^48) range"
                );
            }
        }
        if !reuse || index == 0 {
            image_entries.push((p_addr, words(&call.p)));
            image_entries.push((expected_addr, words(&call.expected)));
            if let Some(q) = &call.q {
                image_entries.push((q_addr, words(q)));
            }
        }

        load_address(&mut instructions, 10, p_addr);
        match call.op.kind {
            Kind::Add => load_address(&mut instructions, 11, q_addr),
            // Doubling does not dereference a1, but the double AIR sends a
            // literal zero as the syscall's second operand while the CPU sends
            // the actual register, so `a1 == 0` is required for provability.
            Kind::Double => load_address(&mut instructions, 11, double_a1),
        }
        let syscall = call.op.syscall as u32 as u64;
        instructions.push(Instruction::new(
            Opcode::LUI,
            5,
            syscall & !0xfff,
            syscall & !0xfff,
            true,
            true,
        ));
        instructions.push(Instruction::new(
            Opcode::ADDI,
            5,
            5,
            syscall & 0xfff,
            false,
            true,
        ));
        instructions.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));

        // Register-only filler: adds guest cycles without any memory access, so a
        // sweep over `pad_alu` separates the per-cycle cost of the global
        // interaction chip from its per-memory-access cost.
        for _ in 0..pad_alu {
            instructions.push(Instruction::new(Opcode::ADDI, 22, 22, 1, false, true));
        }
        if compare {
            load_address(&mut instructions, 12, expected_addr);
            for limb in 0..8u64 {
                instructions.push(Instruction::new(Opcode::LD, 20, 10, 8 * limb, false, true));
                instructions.push(Instruction::new(Opcode::LD, 21, 12, 8 * limb, false, true));
                branches.push(instructions.len());
                instructions.push(Instruction::new(Opcode::BNE, 20, 21, 0, false, true));
            }
        }

        call_json.push(json!({
            "index": index,
            "operation": call.op.id,
            "syscall_code": format!("0x{:08x}", call.op.syscall as u32),
            "p_address": p_addr,
            "q_address": call.q.as_ref().map(|_| q_addr),
            "expected_address": expected_addr,
            "p_words_le": words(&call.p),
            "q_words_le": call.q.as_ref().map(words),
            "expected_words_le": words(&call.expected),
        }));
    }

    if let Some(tag) = commit_tag {
        add_commitments(&mut instructions, tag);
    }
    if halt_exit_code == 0 {
        add_halt(&mut instructions);
    } else {
        // HALT with a nonzero exit code: t0 = HALT (0), a0 = the exit code.
        instructions.push(Instruction::new(Opcode::ADDI, 5, 0, 0, false, true));
        load_address(&mut instructions, 10, halt_exit_code);
        instructions.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    }
    let failure = instructions.len();
    instructions.push(Instruction::new(Opcode::UNIMP, 0, 0, 0, false, false));
    for i in branches {
        instructions[i].op_c = ((failure - i) * 4) as u64;
    }

    ensure!(
        0x2000 + 4 * instructions.len() as u64 <= base_address,
        "instruction region ({} instructions from 0x2000) would overlap the data base 0x{:x}",
        instructions.len(),
        base_address
    );
    let mut program = Program::new(instructions, 0x2000, 0x2000);
    let image = Arc::make_mut(&mut program.memory_image);
    for (address, values) in image_entries {
        for (i, word) in values.into_iter().enumerate() {
            image.insert(address + 8 * i as u64, word);
        }
    }
    let fixture = json!({
        "calls": call_json,
        "instructions": program.instructions.len(),
        "compiled_elf": false,
        "output_comparisons_emitted": compare,
        "data_base_address": base_address,
        "memory_slots_reused": reuse,
        "alu_filler_per_call": pad_alu,
        "commitments_emitted": commit_tag.is_some(),
        "double_a1": double_a1,
        "halt_exit_code": halt_exit_code,
        "committed_digest_hex": commit_tag.map(|t| hex::encode(fixture_digest_bytes(t))),
        "reference_arithmetic": "independent Fermat inversion, separate from the executor and the AIR witness generator",
        "input_validity_binding": "fixed canonical on-curve inputs and expected outputs are constants in the program memory image committed by the verification key",
        "program_checks": "eight LD/BNE comparisons per call require every output word to equal its expected constant before a successful HALT",
        "raw_abi": "t0 = syscall code, a0 = in-place first point, a1 = second point (add) or unused (double); each point is four little-endian x words then four y words, 8-byte aligned",
    });
    Ok((Arc::new(program), fixture))
}

// ---------------------------------------------------------------------------
// Mutations. Each returns evidence that the committed witness really changed.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Case {
    Honest,
    ResultX3,
    ResultY3,
    Slope,
    SlopeDenominator,
    InverseCheck,
    MemoryTimestamp,
    MemoryValue,
    MemoryAddress,
    DropEvent,
    DuplicateEvent,
}

impl Case {
    fn id(self) -> &'static str {
        match self {
            Case::Honest => "honest",
            Case::ResultX3 => "corrupted-result-x3",
            Case::ResultY3 => "corrupted-result-y3",
            Case::Slope => "corrupted-slope",
            Case::SlopeDenominator => "corrupted-slope-denominator",
            Case::InverseCheck => "corrupted-inverse-check",
            Case::MemoryTimestamp => "forged-memory-timestamp",
            Case::MemoryValue => "altered-memory-value",
            Case::MemoryAddress => "altered-memory-address",
            Case::DropEvent => "dropped-precompile-event",
            Case::DuplicateEvent => "duplicated-precompile-event",
        }
    }

    /// Which constraint or interaction the case is intended to exercise.
    fn covers(self) -> &'static str {
        match self {
            Case::Honest => "no mutation; honest acceptance control",
            Case::ResultX3 => "x3 = slope^2 - (x1 + x2) field relation and the output memory write binding",
            Case::ResultY3 => "y3 = slope*(x1 - x3) - y1 field relation and the output memory write binding",
            Case::Slope => "slope * denominator = numerator division relation",
            Case::SlopeDenominator => "denominator field relation (q.x - p.x for add, 2y for double)",
            Case::InverseCheck => "inverse_check * denominator = 1, the add chip's nonzero-denominator constraint",
            Case::MemoryTimestamp => "memory argument's previous-timestamp binding, with this chip's byte/range dependencies refreshed",
            Case::MemoryValue => "memory argument's value binding between the precompile row and authenticated memory",
            Case::MemoryAddress => "memory argument's address binding and the syscall address operands",
            Case::DropEvent => "CPU-to-precompile syscall interaction: a syscall with no precompile row",
            Case::DuplicateEvent => "CPU-to-precompile syscall interaction: a precompile row with no syscall",
        }
    }

    /// Which layer must reject this case. Every witness corruption has to be
    /// caught by the cryptography: if one were ever rejected by a structural or
    /// policy check instead, the case would still satisfy an
    /// accepted == expected gate while no longer testing what it claims to.
    fn expected_rejection_check(self) -> Option<&'static str> {
        match self {
            Case::Honest => None,
            _ => Some("shard_cryptographic_verification"),
        }
    }

    fn is_trace_mutation(self) -> bool {
        matches!(
            self,
            Case::ResultX3
                | Case::ResultY3
                | Case::Slope
                | Case::SlopeDenominator
                | Case::InverseCheck
        )
    }
}

fn cases_for(op: &Op) -> Vec<Case> {
    let mut cases = vec![
        Case::Honest,
        Case::ResultX3,
        Case::ResultY3,
        Case::Slope,
        Case::SlopeDenominator,
    ];
    if op.kind == Kind::Add {
        // Only the add chip has an inverse_check column.
        cases.push(Case::InverseCheck);
    }
    cases.extend([
        Case::MemoryTimestamp,
        Case::MemoryValue,
        Case::MemoryAddress,
        Case::DropEvent,
        Case::DuplicateEvent,
    ]);
    cases
}

/// Flip one bit of a byte limb in the first real row of the chip's trace,
/// after witness generation and before cryptographic commitment.
fn mutate_trace(data: &mut TraceData, op: &Op, case: Case) -> Result<Option<Value>> {
    let Some(trace) = data.traces.remove(op.chip) else {
        return Ok(None);
    };
    if trace.num_real_entries() == 0 {
        data.traces.insert(op.chip.to_owned(), trace);
        return Ok(None);
    }
    let variables = trace.num_variables();
    let padding = trace.clone().into_padding_values();
    let mut mle = (*trace.into_inner().context("chip trace missing data")?).clone();
    let before_bytes = bincode::serialize(mle.guts().as_slice())?;
    let width = op_width(op);
    let values = mle.guts_mut().as_mut_slice();
    ensure!(values.len() >= width, "trace shorter than one row");
    let row = &mut values[..width];

    macro_rules! pick {
        ($cols:ty) => {{
            let cols: &mut $cols = row.borrow_mut();
            ensure!(
                cols.is_real == SP1Field::one(),
                "first {} row is not real",
                op.chip
            );
            match case {
                Case::ResultX3 => &mut cols.x3_ins.result.0[0],
                Case::ResultY3 => &mut cols.y3_ins.result.0[0],
                Case::Slope => &mut cols.slope.result.0[0],
                Case::SlopeDenominator => &mut cols.slope_denominator.result.0[0],
                _ => bail!("{:?} is not a trace mutation for {}", case, op.chip),
            }
        }};
    }

    let (before, after, target) = match (op.curve, op.kind, case) {
        (Curve::Vesta, Kind::Add, Case::InverseCheck) => {
            let cols: &mut WeierstrassAddAssignCols<
                SP1Field,
                VestaBaseField,
                sp1_core_machine::SupervisorMode,
            > = row.borrow_mut();
            let limb = &mut cols.inverse_check.result.0[0];
            let before = limb.as_canonical_u32();
            let after = before ^ 1;
            *limb = SP1Field::from_canonical_u32(after);
            (before, after, "inverse_check.result byte limb 0")
        }
        (Curve::Pallas, Kind::Add, Case::InverseCheck) => {
            let cols: &mut WeierstrassAddAssignCols<
                SP1Field,
                PallasBaseField,
                sp1_core_machine::SupervisorMode,
            > = row.borrow_mut();
            let limb = &mut cols.inverse_check.result.0[0];
            let before = limb.as_canonical_u32();
            let after = before ^ 1;
            *limb = SP1Field::from_canonical_u32(after);
            (before, after, "inverse_check.result byte limb 0")
        }
        (Curve::Vesta, Kind::Add, _) => {
            let limb = pick!(WeierstrassAddAssignCols<SP1Field, VestaBaseField, sp1_core_machine::SupervisorMode>);
            let before = limb.as_canonical_u32();
            let after = before ^ 1;
            *limb = SP1Field::from_canonical_u32(after);
            (before, after, trace_target_name(case))
        }
        (Curve::Pallas, Kind::Add, _) => {
            let limb = pick!(WeierstrassAddAssignCols<SP1Field, PallasBaseField, sp1_core_machine::SupervisorMode>);
            let before = limb.as_canonical_u32();
            let after = before ^ 1;
            *limb = SP1Field::from_canonical_u32(after);
            (before, after, trace_target_name(case))
        }
        (Curve::Vesta, Kind::Double, _) => {
            let limb = pick!(WeierstrassDoubleAssignCols<SP1Field, VestaBaseField, sp1_core_machine::SupervisorMode>);
            let before = limb.as_canonical_u32();
            let after = before ^ 1;
            *limb = SP1Field::from_canonical_u32(after);
            (before, after, trace_target_name(case))
        }
        (Curve::Pallas, Kind::Double, _) => {
            let limb = pick!(WeierstrassDoubleAssignCols<SP1Field, PallasBaseField, sp1_core_machine::SupervisorMode>);
            let before = limb.as_canonical_u32();
            let after = before ^ 1;
            *limb = SP1Field::from_canonical_u32(after);
            (before, after, trace_target_name(case))
        }
    };

    ensure!(before < 256, "expected a byte limb, got {before}");
    let after_bytes = bincode::serialize(mle.guts().as_slice())?;
    ensure!(before_bytes != after_bytes, "trace mutation had no effect");
    data.traces.insert(
        op.chip.to_owned(),
        PaddedMle::padded(Arc::new(mle), variables, padding),
    );
    Ok(Some(json!({
        "target": format!("{} row 0 {}", op.chip, target),
        "before": before,
        "after": after,
        "mutation_stage": "after witness/trace generation, before cryptographic commitment",
        "trace_sha256_before": sha(&before_bytes),
        "trace_sha256_after": sha(&after_bytes),
    })))
}

fn trace_target_name(case: Case) -> &'static str {
    match case {
        Case::ResultX3 => "x3_ins.result byte limb 0",
        Case::ResultY3 => "y3_ins.result byte limb 0",
        Case::Slope => "slope.result byte limb 0",
        Case::SlopeDenominator => "slope_denominator.result byte limb 0",
        Case::InverseCheck => "inverse_check.result byte limb 0",
        _ => "unknown",
    }
}

/// Mutate the execution record, then refresh only this chip's byte/range
/// lookup multiplicities so a rejection cannot be explained by stale counts
/// alone. The authenticated global memory history is left untouched.
fn mutate_record(record: &mut ExecutionRecord, op: &Op, case: Case) -> Result<Option<Value>> {
    let Some(events) = record.precompile_events.events.get(&op.syscall) else {
        return Ok(None);
    };
    if events.is_empty() {
        return Ok(None);
    }
    ensure!(
        events.len() == 1,
        "expected exactly one {} event per shard",
        op.id
    );

    let (trace_before, deps_before) = chip_trace_and_deps(record, op)?;
    let serialized_before = bincode::serialize(&record)?;

    let detail = {
        let slot = record
            .precompile_events
            .events
            .get_mut(&op.syscall)
            .unwrap();
        match case {
            Case::DropEvent => {
                slot.clear();
                json!({"target": format!("{} precompile event list", op.id), "action": "removed the only event", "events_after": 0})
            }
            Case::DuplicateEvent => {
                let first = slot[0].clone();
                slot.push(first);
                json!({"target": format!("{} precompile event list", op.id), "action": "appended a byte-identical copy of the only event", "events_after": 2})
            }
            _ => {
                let (_, event) = &mut slot[0];
                match op.kind {
                    Kind::Add => {
                        let (PrecompileEvent::VestaAdd(e) | PrecompileEvent::PallasAdd(e)) = event
                        else {
                            bail!("wrong add event variant for {}", op.id);
                        };
                        mutate_add_event(e, case)?
                    }
                    Kind::Double => {
                        let (PrecompileEvent::VestaDouble(e) | PrecompileEvent::PallasDouble(e)) =
                            event
                        else {
                            bail!("wrong double event variant for {}", op.id);
                        };
                        mutate_double_event(e, case)?
                    }
                }
            }
        }
    };

    refresh_dependencies(record, op, deps_before)?;
    let serialized_after = bincode::serialize(&record)?;
    ensure!(
        serialized_before != serialized_after,
        "record mutation had no effect"
    );

    let (trace_after, _) = chip_trace_and_deps(record, op)?;
    let changed_rows = case == Case::DropEvent || case == Case::DuplicateEvent;
    if !changed_rows {
        ensure!(
            trace_before != trace_after,
            "record mutation did not change the committed {} witness trace",
            op.chip
        );
    }

    let mut evidence = detail;
    let object = evidence.as_object_mut().unwrap();
    object.insert("byte_and_range_dependencies_recomputed".into(), json!(true));
    object.insert(
        "honest_memory_boundary_records_preserved".into(),
        json!(true),
    );
    object.insert(
        "record_sha256_before".into(),
        json!(sha(&serialized_before)),
    );
    object.insert("record_sha256_after".into(), json!(sha(&serialized_after)));
    object.insert("trace_sha256_before".into(), json!(sha(&trace_before)));
    object.insert("trace_sha256_after".into(), json!(sha(&trace_after)));
    object.insert("row_count_changed".into(), json!(changed_rows));
    Ok(Some(evidence))
}

fn mutate_add_event(event: &mut EllipticCurveAddEvent, case: Case) -> Result<Value> {
    match case {
        Case::MemoryTimestamp => {
            let access = &mut event.q_memory_records[0];
            ensure!(
                access.prev_timestamp == 0 && access.timestamp > 8,
                "fixture must read q first, after timestamp 8"
            );
            let current = access.timestamp;
            access.prev_timestamp = 8;
            Ok(json!({
                "target": "q_memory_records[0].prev_timestamp",
                "before": 0, "after": 8, "current_timestamp": current,
            }))
        }
        Case::MemoryValue => {
            let access = &mut event.q_memory_records[0];
            let before = access.value;
            access.value = before ^ 1;
            event.q[0] = access.value;
            Ok(json!({
                "target": "q_memory_records[0].value and the mirrored q word",
                "before": before, "after": access.value,
            }))
        }
        Case::MemoryAddress => {
            let before = event.q_ptr;
            // Stay 8-byte aligned and inside SP1's valid data range.
            event.q_ptr = before + 8;
            Ok(json!({
                "target": "q_ptr", "before": before, "after": event.q_ptr,
            }))
        }
        other => bail!("{other:?} is not an add-event mutation"),
    }
}

fn mutate_double_event(event: &mut EllipticCurveDoubleEvent, case: Case) -> Result<Value> {
    match case {
        Case::MemoryTimestamp => {
            let access = &mut event.p_memory_records[0];
            ensure!(
                access.prev_timestamp == 0,
                "fixture must write p from timestamp 0"
            );
            let current = access.timestamp;
            ensure!(current > 8, "fixture write timestamp must exceed 8");
            access.prev_timestamp = 8;
            Ok(json!({
                "target": "p_memory_records[0].prev_timestamp",
                "before": 0, "after": 8, "current_timestamp": current,
            }))
        }
        Case::MemoryValue => {
            let access = &mut event.p_memory_records[0];
            let before = access.prev_value;
            access.prev_value = before ^ 1;
            event.p[0] = access.prev_value;
            Ok(json!({
                "target": "p_memory_records[0].prev_value and the mirrored p word",
                "before": before, "after": access.prev_value,
            }))
        }
        Case::MemoryAddress => {
            let before = event.p_ptr;
            event.p_ptr = before + 8;
            Ok(json!({
                "target": "p_ptr", "before": before, "after": event.p_ptr,
            }))
        }
        other => bail!("{other:?} is not a double-event mutation"),
    }
}

/// Serialize the chip's generated trace and return its byte/range dependencies.
/// Running SHA-256 digests of one Pasta chip's witness over a whole run: the
/// trace values in row-major order, and each record's byte lookups sorted so
/// the digest does not depend on HashMap iteration order.
#[derive(Default)]
struct WitnessDigest {
    trace: Sha256,
    dependencies: Sha256,
    trace_values: u64,
    lookup_entries: u64,
}

impl WitnessDigest {
    fn absorb(&mut self, values: &[SP1Field], dependencies: &ExecutionRecord) {
        // Hash in bounded pieces: a single chip trace can be gigabytes.
        let mut buf = Vec::with_capacity(1 << 20);
        for chunk in values.chunks(1 << 18) {
            buf.clear();
            for v in chunk {
                buf.extend_from_slice(&v.as_canonical_u32().to_le_bytes());
            }
            self.trace.update(&buf);
        }
        self.trace_values += values.len() as u64;

        let mut lookups: Vec<Vec<u8>> = dependencies
            .byte_lookups
            .iter()
            .map(|entry| bincode::serialize(&entry).expect("byte lookup serializes"))
            .collect();
        lookups.sort_unstable();
        for lookup in &lookups {
            self.dependencies.update(lookup);
        }
        self.lookup_entries += lookups.len() as u64;
    }

    fn finish(self) -> Value {
        json!({
            "trace_sha256": hex::encode(self.trace.finalize()),
            "trace_values": self.trace_values,
            "dependencies_sha256": hex::encode(self.dependencies.finalize()),
            "lookup_entries": self.lookup_entries,
        })
    }
}

/// Time one Pasta chip's dependency and trace generation on a real record,
/// optionally folding both outputs into `digest`. Returns `(deps, trace)` seconds.
fn time_chip_witness(
    record: &ExecutionRecord,
    op: &Op,
    digest: Option<&mut WitnessDigest>,
) -> (f64, f64) {
    use sp1_core_machine::SupervisorMode;
    match (op.curve, op.kind) {
        (Curve::Vesta, Kind::Add) => time_witness_for(
            &WeierstrassAddAssignChip::<Vesta, SupervisorMode>::new(),
            record,
            digest,
        ),
        (Curve::Pallas, Kind::Add) => time_witness_for(
            &WeierstrassAddAssignChip::<Pallas, SupervisorMode>::new(),
            record,
            digest,
        ),
        (Curve::Vesta, Kind::Double) => time_witness_for(
            &WeierstrassDoubleAssignChip::<Vesta, SupervisorMode>::new(),
            record,
            digest,
        ),
        (Curve::Pallas, Kind::Double) => time_witness_for(
            &WeierstrassDoubleAssignChip::<Pallas, SupervisorMode>::new(),
            record,
            digest,
        ),
    }
}

fn time_witness_for<C: MachineAir<SP1Field, Record = ExecutionRecord>>(
    chip: &C,
    record: &ExecutionRecord,
    digest: Option<&mut WitnessDigest>,
) -> (f64, f64) {
    let mut dependencies = ExecutionRecord::default();
    let t = Instant::now();
    chip.generate_dependencies(record, &mut dependencies);
    let deps_seconds = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let trace = chip.generate_trace(record, &mut ExecutionRecord::default());
    let trace_seconds = t.elapsed().as_secs_f64();
    if let Some(digest) = digest {
        digest.absorb(&trace.values, &dependencies);
    }
    (deps_seconds, trace_seconds)
}

fn chip_trace_and_deps(record: &ExecutionRecord, op: &Op) -> Result<(Vec<u8>, ExecutionRecord)> {
    let mut deps = ExecutionRecord::default();
    let mut input = record.clone();
    let bytes = match (op.curve, op.kind) {
        (Curve::Vesta, Kind::Add) => {
            let chip = WeierstrassAddAssignChip::<Vesta, sp1_core_machine::SupervisorMode>::new();
            MachineAir::<SP1Field>::generate_dependencies(&chip, &input, &mut deps);
            let t = MachineAir::<SP1Field>::generate_trace(
                &chip,
                &mut input,
                &mut ExecutionRecord::default(),
            );
            bincode::serialize(&t.values)?
        }
        (Curve::Pallas, Kind::Add) => {
            let chip = WeierstrassAddAssignChip::<Pallas, sp1_core_machine::SupervisorMode>::new();
            MachineAir::<SP1Field>::generate_dependencies(&chip, &input, &mut deps);
            let t = MachineAir::<SP1Field>::generate_trace(
                &chip,
                &mut input,
                &mut ExecutionRecord::default(),
            );
            bincode::serialize(&t.values)?
        }
        (Curve::Vesta, Kind::Double) => {
            let chip =
                WeierstrassDoubleAssignChip::<Vesta, sp1_core_machine::SupervisorMode>::new();
            MachineAir::<SP1Field>::generate_dependencies(&chip, &input, &mut deps);
            let t = MachineAir::<SP1Field>::generate_trace(
                &chip,
                &mut input,
                &mut ExecutionRecord::default(),
            );
            bincode::serialize(&t.values)?
        }
        (Curve::Pallas, Kind::Double) => {
            let chip =
                WeierstrassDoubleAssignChip::<Pallas, sp1_core_machine::SupervisorMode>::new();
            MachineAir::<SP1Field>::generate_dependencies(&chip, &input, &mut deps);
            let t = MachineAir::<SP1Field>::generate_trace(
                &chip,
                &mut input,
                &mut ExecutionRecord::default(),
            );
            bincode::serialize(&t.values)?
        }
    };
    Ok((bytes, deps))
}

fn refresh_dependencies(
    record: &mut ExecutionRecord,
    op: &Op,
    deps_before: ExecutionRecord,
) -> Result<()> {
    let (_, deps_after) = chip_trace_and_deps(record, op)?;
    for (lookup, count) in deps_before.byte_lookups {
        let stored = record
            .byte_lookups
            .get_mut(&lookup)
            .context("missing original byte lookup")?;
        ensure!(*stored >= count, "byte lookup multiplicity underflow");
        *stored -= count;
    }
    for (lookup, count) in deps_after.byte_lookups {
        *record.byte_lookups.entry(lookup).or_default() += count;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Proving.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Complete-execution verification.
//
// `SimpleProver::verify` delegates to `MachineVerifier::verify`, which only
// loops over the supplied shard proofs and verifies each one. It performs no
// continuity, completeness, boundary or cumulative-sum check, so it accepts an
// empty proof, a single shard lifted out of a longer execution, and shards that
// have been dropped, duplicated or reordered.
//
// The checks below are a faithful adaptation of the pinned SP1 6.1.0 production
// core verifier, `SP1Prover::verify` in `crates/prover/src/verify.rs`, in the
// same order, against the trusted verification key. Per-shard cryptographic
// verification runs last, exactly as it does there.
// ---------------------------------------------------------------------------

type Pv = PublicValues<[SP1Field; 4], [SP1Field; 3], [SP1Field; 4], SP1Field>;

#[derive(Debug, Clone)]
struct CompleteError {
    /// Stable identifier so negative tests can assert *which* check rejected.
    check: &'static str,
    detail: String,
}

impl CompleteError {
    fn new(check: &'static str, detail: impl Into<String>) -> Self {
        Self {
            check,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for CompleteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.check, self.detail)
    }
}

/// Application policy layered on top of the canonical checks.
#[derive(Debug, Clone, Default)]
struct Policy {
    /// Require `exit_code == 0` on the final shard.
    require_successful_exit: bool,
    /// Require this exact committed public-value digest.
    expected_committed_digest: Option<[u8; 32]>,
}

fn pv_of(shard: &sp1_hypercube::ShardProof<SP1GlobalContext, SP1PcsProofInner>) -> &Pv {
    shard.public_values.as_slice().borrow()
}

fn digest_bytes(pv: &Pv) -> std::result::Result<[u8; 32], CompleteError> {
    let mut out = [0u8; 32];
    for (i, word) in pv.committed_value_digest.iter().enumerate() {
        for (j, byte) in word.iter().enumerate() {
            let value = byte.as_canonical_u32();
            // Reject rather than mask: a masking decode would let a limb of
            // `expected + 256` alias past the policy comparison.
            if value >= 256 {
                return Err(CompleteError::new(
                    "committed_digest_limb_range",
                    format!("committed_value_digest[{i}][{j}] = {value} is not a byte"),
                ));
            }
            out[4 * i + j] = value as u8;
        }
    }
    Ok(out)
}

/// Structural checks only; cryptographic shard verification is the caller's
/// final step. Split out so a test can report which layer rejected.
fn check_execution_structure(
    vk: &MachineVerifyingKey<SP1GlobalContext>,
    proof: &Proof,
) -> Result<(), CompleteError> {
    let shards = &proof.shard_proofs;

    // Non-empty.
    if shards.is_empty() {
        return Err(CompleteError::new(
            "empty_proof",
            "the proof contains no shards",
        ));
    }

    // Public-values length.
    for (i, shard) in shards.iter().enumerate() {
        if shard.public_values.len() != sp1_hypercube::PROOF_MAX_NUM_PVS {
            return Err(CompleteError::new(
                "public_values_length",
                format!(
                    "shard {i} has {} public values, expected {}",
                    shard.public_values.len(),
                    sp1_hypercube::PROOF_MAX_NUM_PVS
                ),
            ));
        }
    }

    let values: Vec<Pv> = shards.iter().map(|shard| *pv_of(shard)).collect();
    check_public_value_chain(vk, &values)
}

fn check_public_value_chain(
    vk: &MachineVerifyingKey<SP1GlobalContext>,
    shards: &[Pv],
) -> Result<(), CompleteError> {
    if shards.is_empty() {
        return Err(CompleteError::new("empty_proof", "no public-value shards"));
    }
    // `is_first_execution_shard` is boolean and set for exactly one shard.
    let mut first_set = false;
    for (i, shard) in shards.iter().enumerate() {
        let pv = shard;
        if pv.is_first_execution_shard == SP1Field::one() {
            if first_set {
                return Err(CompleteError::new(
                    "first_execution_shard_unique",
                    format!("is_first_execution_shard is set on more than one shard, again at {i}"),
                ));
            }
            first_set = true;
        } else if pv.is_first_execution_shard != SP1Field::zero() {
            return Err(CompleteError::new(
                "first_execution_shard_boolean",
                format!("is_first_execution_shard is not boolean on shard {i}"),
            ));
        }
    }
    if !first_set {
        return Err(CompleteError::new(
            "first_execution_shard_present",
            "no shard is marked as the first execution shard",
        ));
    }

    // Timestamp chain.
    let mut prev_timestamp = [
        SP1Field::zero(),
        SP1Field::zero(),
        SP1Field::zero(),
        SP1Field::one(),
    ];
    for (i, shard) in shards.iter().enumerate() {
        let pv = shard;
        if pv.initial_timestamp != prev_timestamp {
            return Err(CompleteError::new(
                "timestamp_chain",
                format!("shard {i} initial_timestamp does not continue the previous shard"),
            ));
        }
        if pv.is_execution_shard != SP1Field::zero() && pv.initial_timestamp == pv.last_timestamp {
            return Err(CompleteError::new(
                "timestamp_moves_on_execution_shard",
                format!("shard {i} is an execution shard but its timestamp did not move"),
            ));
        }
        if pv.is_execution_shard != SP1Field::one() && pv.initial_timestamp != pv.last_timestamp {
            return Err(CompleteError::new(
                "timestamp_static_on_non_execution_shard",
                format!("shard {i} is not an execution shard but its timestamp moved"),
            ));
        }
        prev_timestamp = pv.last_timestamp;
    }

    // Program-counter chain, ending at HALT_PC.
    let mut prev_next_pc = vk.pc_start;
    let halt_pc = [
        SP1Field::from_canonical_u64(sp1_core_executor::HALT_PC),
        SP1Field::zero(),
        SP1Field::zero(),
    ];
    for (i, shard) in shards.iter().enumerate() {
        let pv = shard;
        if pv.pc_start != prev_next_pc {
            return Err(CompleteError::new(
                if i == 0 {
                    "pc_start_matches_vk"
                } else {
                    "pc_chain"
                },
                format!("shard {i} pc_start does not continue the previous next_pc"),
            ));
        }
        if pv.is_execution_shard != SP1Field::one() && pv.pc_start != pv.next_pc {
            return Err(CompleteError::new(
                "pc_static_on_non_execution_shard",
                format!("shard {i} is not an execution shard but its pc moved"),
            ));
        }
        prev_next_pc = pv.next_pc;
    }
    if prev_next_pc != halt_pc {
        return Err(CompleteError::new(
            "execution_halted",
            "the final next_pc is not HALT_PC, so the execution did not halt",
        ));
    }

    // Exit-code chain.
    let mut prev_exit_code = SP1Field::zero();
    for (i, shard) in shards.iter().enumerate() {
        let pv = shard;
        if pv.prev_exit_code != prev_exit_code {
            return Err(CompleteError::new(
                "exit_code_chain",
                format!("shard {i} prev_exit_code does not match the previous shard's exit_code"),
            ));
        }
        if pv.is_execution_shard != SP1Field::one() && pv.prev_exit_code != pv.exit_code {
            return Err(CompleteError::new(
                "exit_code_static_on_non_execution_shard",
                format!("shard {i} is not an execution shard but its exit code changed"),
            ));
        }
        if pv.prev_exit_code != SP1Field::zero() && pv.prev_exit_code != pv.exit_code {
            return Err(CompleteError::new(
                "exit_code_changes_once",
                format!("shard {i} changes an already non-zero exit code"),
            ));
        }
        prev_exit_code = pv.exit_code;
    }

    // SP1 6.10 requires successful execution in its non-experimental verifier.
    if prev_exit_code != SP1Field::zero() {
        return Err(CompleteError::new(
            "canonical_successful_exit",
            "SP1 6.10 rejects nonzero exits without experimental verification",
        ));
    }

    // Proof nonce is constant.
    let nonce = shards[0].proof_nonce;
    for (i, shard) in shards.iter().enumerate().skip(1) {
        if shard.proof_nonce != nonce {
            return Err(CompleteError::new(
                "proof_nonce_constant",
                format!("shard {i} carries a different proof nonce"),
            ));
        }
    }

    // Memory and page initialization/finalization chains.
    let mut last_init_addr = [SP1Field::zero(); 3];
    let mut last_finalize_addr = [SP1Field::zero(); 3];
    let mut last_init_page = [SP1Field::zero(); 3];
    let mut last_finalize_page = [SP1Field::zero(); 3];
    for (i, shard) in shards.iter().enumerate() {
        let pv = shard;
        if pv.previous_init_addr != last_init_addr {
            return Err(CompleteError::new(
                "memory_init_chain",
                format!("shard {i} previous_init_addr breaks the memory initialization chain"),
            ));
        }
        if pv.previous_finalize_addr != last_finalize_addr {
            return Err(CompleteError::new(
                "memory_finalize_chain",
                format!("shard {i} previous_finalize_addr breaks the finalization chain"),
            ));
        }
        if pv.previous_init_page_idx != last_init_page {
            return Err(CompleteError::new(
                "page_init_chain",
                format!("shard {i} previous_init_page_idx breaks the page initialization chain"),
            ));
        }
        if pv.previous_finalize_page_idx != last_finalize_page {
            return Err(CompleteError::new(
                "page_finalize_chain",
                format!("shard {i} previous_finalize_page_idx breaks the page finalization chain"),
            ));
        }
        if pv.is_untrusted_programs_enabled != vk.untrusted_config.enable_untrusted_programs {
            return Err(CompleteError::new(
                "untrusted_programs_flag",
                format!("shard {i} disagrees with the verification key on untrusted programs"),
            ));
        }
        last_init_addr = pv.last_init_addr;
        last_finalize_addr = pv.last_finalize_addr;
        last_init_page = pv.last_init_page_idx;
        last_finalize_page = pv.last_finalize_page_idx;
    }
    if last_init_addr == [SP1Field::zero(); 3] {
        return Err(CompleteError::new(
            "memory_initialized",
            "no address was ever initialized",
        ));
    }
    if last_finalize_addr == [SP1Field::zero(); 3] {
        return Err(CompleteError::new(
            "memory_finalized",
            "no address was ever finalized",
        ));
    }

    #[cfg(feature = "mprotect")]
    for (i, shard) in shards.iter().enumerate() {
        let pv = shard;
        if pv.trap_context != vk.untrusted_config.trap_context {
            return Err(CompleteError::new(
                "trap_context_matches_vk",
                format!("shard {i} trap context disagrees with key"),
            ));
        }
        if pv.untrusted_memory != vk.untrusted_config.untrusted_memory {
            return Err(CompleteError::new(
                "untrusted_memory_matches_vk",
                format!("shard {i} memory configuration disagrees with key"),
            ));
        }
        if pv.enable_trap_handler != vk.untrusted_config.enable_trap_handler {
            return Err(CompleteError::new(
                "trap_handler_matches_vk",
                format!("shard {i} trap-handler flag disagrees with key"),
            ));
        }
    }

    // Commitment chains, and that both commit syscalls actually ran.
    let mut committed_prev = [[SP1Field::zero(); 4]; sp1_hypercube::air::PV_DIGEST_NUM_WORDS];
    let mut deferred_prev = [SP1Field::zero(); sp1_hypercube::air::POSEIDON_NUM_WORDS];
    let mut commit_prev = SP1Field::zero();
    let mut commit_deferred_prev = SP1Field::zero();
    for (i, shard) in shards.iter().enumerate() {
        let pv = shard;
        if pv.prev_committed_value_digest != committed_prev {
            return Err(CompleteError::new(
                "committed_digest_chain",
                format!("shard {i} breaks the committed-value digest chain"),
            ));
        }
        if pv.prev_deferred_proofs_digest != deferred_prev {
            return Err(CompleteError::new(
                "deferred_digest_chain",
                format!("shard {i} breaks the deferred-proofs digest chain"),
            ));
        }
        if pv.prev_commit_syscall != commit_prev {
            return Err(CompleteError::new(
                "commit_syscall_chain",
                format!("shard {i} breaks the commit-syscall chain"),
            ));
        }
        if pv.prev_commit_deferred_syscall != commit_deferred_prev {
            return Err(CompleteError::new(
                "commit_deferred_syscall_chain",
                format!("shard {i} breaks the deferred commit-syscall chain"),
            ));
        }
        committed_prev = pv.committed_value_digest;
        deferred_prev = pv.deferred_proofs_digest;
        commit_prev = pv.commit_syscall;
        commit_deferred_prev = pv.commit_deferred_syscall;
    }
    if commit_prev != SP1Field::one() {
        return Err(CompleteError::new(
            "commit_syscall_called",
            "the COMMIT syscall was never called",
        ));
    }
    if commit_deferred_prev != SP1Field::one() {
        return Err(CompleteError::new(
            "commit_deferred_syscall_called",
            "the COMMIT_DEFERRED_PROOFS syscall was never called",
        ));
    }

    // Shard-count bound.
    if shards.len() >= 1 << sp1_core_machine::riscv::MAX_LOG_NUMBER_OF_SHARDS {
        return Err(CompleteError::new("too_many_shards", "too many shards"));
    }

    // Global cumulative sum must close against the verification key.
    let mut cumulative = vk.initial_global_cumulative_sum;
    for shard in shards.iter() {
        cumulative = cumulative
            .checked_add(shard.global_cumulative_sum)
            .ok_or_else(|| {
                CompleteError::new(
                    "global_cumulative_sum_exception",
                    "exceptional global-curve addition",
                )
            })?;
    }
    if !cumulative.is_zero() {
        return Err(CompleteError::new(
            "global_cumulative_sum",
            "the global cumulative sum does not close to zero",
        ));
    }

    Ok(())
}

/// Structural checks, then application policy, then the cryptographic
/// verification of every shard against the trusted verification key.
fn verify_complete_execution(
    prover: &SimpleProver<SP1GlobalContext, InnerSC<Air>, Cpu>,
    vk: &MachineVerifyingKey<SP1GlobalContext>,
    proof: &Proof,
    policy: &Policy,
) -> Result<(), CompleteError> {
    check_execution_structure(vk, proof)?;

    let final_pv = pv_of(proof.shard_proofs.last().expect("non-empty checked above"));
    if policy.require_successful_exit && final_pv.exit_code != SP1Field::zero() {
        return Err(CompleteError::new(
            "successful_exit",
            format!(
                "final exit code is {}",
                final_pv.exit_code.as_canonical_u32()
            ),
        ));
    }
    if let Some(expected) = policy.expected_committed_digest {
        let actual = digest_bytes(final_pv)?;
        if actual != expected {
            return Err(CompleteError::new(
                "expected_committed_digest",
                format!(
                    "committed digest {} does not match the expected {}",
                    hex::encode(actual),
                    hex::encode(expected)
                ),
            ));
        }
    }

    prover
        .verify(vk, proof)
        .map_err(|e| CompleteError::new("shard_cryptographic_verification", format!("{e:?}")))
}

/// Order generated shard proofs into the canonical execution chain, losslessly.
///
/// `generate_records` emits each chunk's execution record followed by its
/// deferred records, but the verifiable order is the public-value chain. A
/// precompile shard does not move the clock, so it chains *before* the
/// execution shard it was deferred from; the memory initialize/finalize shard
/// chains last.
///
/// Sorting by (initial_timestamp, last_timestamp, previous_init_addr,
/// previous_finalize_addr) produces that order. The previous code used this
/// same key in a `BTreeMap`, which ordered correctly but silently dropped every
/// shard whose key collided with another's — distinct precompile shards share
/// the tuple exactly. The fix is to keep the ordering and lose the
/// deduplication: a stable sort over a vector, so tied shards keep generation
/// order and the result is always a permutation of the input.
///
/// Timestamp limbs are most-significant-first, so they compare lexicographically.
/// Address limbs are little-endian 16-bit, so they are decoded before comparing.
/// The caller verifies the resulting order with the complete verifier, so a
/// wrong tie-break cannot pass silently.
fn canonical_shard_order(
    shards: Vec<sp1_hypercube::ShardProof<SP1GlobalContext, SP1PcsProofInner>>,
) -> Vec<sp1_hypercube::ShardProof<SP1GlobalContext, SP1PcsProofInner>> {
    sp1_core_machine::utils::order_execution_items::<SP1Field, _>(shards, |shard| pv_of(shard))
}

// ---------------------------------------------------------------------------
// Mode: structural
//
// Reproduces, as enforced regressions, every structural attack the independent
// review showed the per-shard endpoint accepting. Each case asserts that the
// complete verifier rejects, and records whether the weaker per-shard endpoint
// would have accepted, so the regression documents the former false pass.
// ---------------------------------------------------------------------------

fn tamper_pv(
    shard: &mut sp1_hypercube::ShardProof<SP1GlobalContext, SP1PcsProofInner>,
    edit: impl FnOnce(&mut Pv),
) {
    let pv: &mut Pv = shard.public_values.as_mut_slice().borrow_mut();
    edit(pv);
}

async fn run_structural(output: &Path) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    fs::create_dir_all(output)?;
    let started = Instant::now();

    // One honest multi-shard execution to attack, and a second, different
    // honest execution to supply foreign shards.
    let add_op = *OPS.iter().find(|o| o.id == "vesta-add").unwrap();
    let dbl_op = *OPS.iter().find(|o| o.id == "vesta-double").unwrap();
    let pts = multiples(Curve::Vesta, 4);
    let calls = vec![
        make_call(add_op, pts[0].clone(), Some(pts[1].clone()))?,
        make_call(dbl_op, pts[2].clone(), None)?,
    ];
    let (program, _) = build_program_at(&calls, FIXTURE_BASE, true, false, 0, Some(0), 0, 0)?;
    let prepared = prepare(program).await?;
    let policy = Policy {
        require_successful_exit: true,
        expected_committed_digest: Some(fixture_digest_bytes(0)),
    };
    let (honest, _) = prove_case(
        &prepared.cpu,
        prepared.pk.clone(),
        &prepared.records,
        &add_op,
        Case::Honest,
    )
    .await?;
    let honest_bytes = bincode::serialize(&honest)?;
    fs::write(output.join("honest.proof.bin"), &honest_bytes)?;
    ensure!(
        verify_complete_execution(&prepared.prover, &prepared.restored_vk, &honest, &policy)
            .is_ok(),
        "the honest control must pass the complete verifier"
    );
    let shard_count = honest.shard_proofs.len();
    ensure!(
        shard_count >= 3,
        "need at least three shards to exercise drop/reorder attacks, got {shard_count}"
    );

    // A second, structurally different honest execution for the mixed case.
    let other_calls = vec![make_call(dbl_op, pts[0].clone(), None)?];
    let (other_program, _) =
        build_program_at(&other_calls, FIXTURE_BASE, true, false, 0, Some(1), 0, 0)?;
    let other = prepare(other_program).await?;
    let (other_proof, _) = prove_case(
        &other.cpu,
        other.pk.clone(),
        &other.records,
        &dbl_op,
        Case::Honest,
    )
    .await?;

    // (id, intent, expected rejecting check, layer that must catch it, proof)
    let mut cases: Vec<(&str, &str, &str, &str, Proof)> = Vec::new();
    cases.push((
        "empty-proof",
        "a proof with no shards at all",
        "empty_proof",
        "structural",
        MachineProof {
            shard_proofs: Vec::new(),
        },
    ));
    cases.push((
        "final-shard-only",
        "only the last shard of a longer execution; its public values are genuine, so a committed-digest comparison alone would not catch it",
        "first_execution_shard_present",
        "structural",
        MachineProof { shard_proofs: vec![honest.shard_proofs.last().unwrap().clone()] },
    ));
    cases.push((
        "first-shard-removed",
        "the first shard dropped; the dropped shard does not move the clock, so the timestamp and pc chains still close and the lookup-bus closure is what catches it",
        "global_cumulative_sum",
        "structural",
        MachineProof { shard_proofs: honest.shard_proofs[1..].to_vec() },
    ));
    {
        let mut v = honest.shard_proofs.clone();
        v.push(honest.shard_proofs.last().unwrap().clone());
        cases.push((
            "duplicated-final-shard",
            "the last shard repeated",
            "memory_init_chain",
            "structural",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = honest.shard_proofs.clone();
        v.reverse();
        cases.push((
            "reversed-shard-order",
            "all shards in reverse order",
            "timestamp_chain",
            "structural",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = honest.shard_proofs.clone();
        v.pop();
        cases.push((
            "final-shard-removed",
            "the last shard dropped",
            "memory_initialized",
            "structural",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = honest.shard_proofs.clone();
        v[0] = other_proof.shard_proofs[0].clone();
        cases.push((
            "mixed-executions",
            "a shard taken from a different execution; proved under another verification key, so the cryptography also rejects it and this case cannot isolate the structural layer",
            "global_cumulative_sum",
            "structural",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = honest.shard_proofs.clone();
        let last = v.len() - 1;
        tamper_pv(&mut v[last], |pv| {
            pv.previous_init_addr[0] = pv.previous_init_addr[0] + SP1Field::one();
        });
        cases.push((
            "broken-memory-boundary",
            "the final shard's previous_init_addr advanced by one; editing public values means the cryptography would also reject, so the structural check is what fires first but not the only net",
            "memory_init_chain",
            "structural",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = honest.shard_proofs.clone();
        let last = v.len() - 1;
        tamper_pv(&mut v[last], |pv| {
            pv.committed_value_digest[0][0] = pv.committed_value_digest[0][0] + SP1Field::one();
        });
        cases.push((
            "altered-commitment",
            "the final shard's committed digest changed. No canonical check covers this: the digest chain compares each shard against the previous one, so tampering the last shard leaves the chain intact. The application policy is what rejects it",
            "expected_committed_digest",
            "policy",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = honest.shard_proofs.clone();
        let last = v.len() - 1;
        tamper_pv(&mut v[last], |pv| pv.exit_code = SP1Field::one());
        cases.push((
            "nonzero-exit-code",
            "the final shard reports a failing exit code; the policy and the cryptography would each also catch this",
            "exit_code_static_on_non_execution_shard",
            "structural",
            MachineProof { shard_proofs: v },
        ));
    }

    let mut results = Vec::new();
    for (id, intent, expected_check, layer, candidate) in cases {
        let complete =
            verify_complete_execution(&prepared.prover, &prepared.restored_vk, &candidate, &policy);
        let per_shard = prepared
            .prover
            .verify(&prepared.restored_vk, &candidate)
            .is_ok();
        let rejected = complete.is_err();
        let check = complete.as_ref().err().map(|e| e.check);
        println!(
            "STRUCTURAL {id} rejected={rejected} check={} expected={expected_check} per_shard_accepted={per_shard}",
            check.unwrap_or("-")
        );
        results.push(json!({
            "case": id,
            "intent": intent,
            "rejecting_layer": layer,
            "shards": candidate.shard_proofs.len(),
            "complete_verifier_rejected": rejected,
            "rejection_check": check,
            "expected_rejection_check": expected_check,
            "rejection_detail": complete.as_ref().err().map(|e| e.detail.clone()),
            "per_shard_verifier_would_accept": per_shard,
            "isolates_structural_layer": layer == "structural" && !per_shard == false,
            "passed": rejected && check == Some(expected_check),
        }));
        ensure!(
            rejected,
            "{id}: the complete verifier accepted a structural attack"
        );
        // Pin the *reason*, not just the rejection. Without this, deleting a
        // structural check would still leave cases that policy or cryptography
        // happens to catch reporting a pass.
        ensure!(
            check == Some(expected_check),
            "{id}: expected rejection at {expected_check}, got {:?}",
            check
        );
    }

    // The documented doubling ABI: a1 must be zero for provability.
    let a1_calls = vec![make_call(dbl_op, pts[0].clone(), None)?];
    let (a1_program, a1_fixture) =
        build_program_at(&a1_calls, FIXTURE_BASE, true, false, 0, Some(0), 8, 0)?;
    let a1 = prepare(a1_program).await?;
    let a1_outcome = match prove_case(&a1.cpu, a1.pk.clone(), &a1.records, &dbl_op, Case::Honest)
        .await
    {
        Ok((proof, _)) => {
            let complete = verify_complete_execution(&a1.prover, &a1.restored_vk, &proof, &policy);
            json!({
                "proof_generated": true,
                "complete_verifier_rejected": complete.is_err(),
                "rejection_check": complete.as_ref().err().map(|e| e.check),
                "rejection_detail": complete.as_ref().err().map(|e| e.detail.clone()),
            })
        }
        Err(error) => json!({"proof_generated": false, "assembly_error": error.to_string()}),
    };
    // A nonzero a1 still executes and still produces a proof; what fails is the
    // syscall interaction. Require exactly that, so an unrelated assembly error
    // cannot make this case report a pass.
    let a1_rejected = a1_outcome["proof_generated"] == json!(true)
        && a1_outcome["complete_verifier_rejected"] == json!(true)
        && a1_outcome["rejection_check"] == json!("shard_cryptographic_verification");
    println!("STRUCTURAL nonzero-double-a1 rejected={a1_rejected}");
    results.push(json!({
        "case": "nonzero-double-a1",
        "rejecting_layer": "cryptographic",
        "expected_rejection_check": "shard_cryptographic_verification",
        "intent": "doubling with a1 = 8: the double AIR sends a literal zero as the syscall's second operand, so a nonzero a1 cannot be proved even though the executor never dereferences it",
        "fixture_double_a1": a1_fixture["double_a1"],
        "outcome": a1_outcome,
        "passed": a1_rejected,
    }));
    ensure!(
        a1_rejected,
        "nonzero-double-a1: expected a generated proof rejected at \
         shard_cryptographic_verification, got {a1_outcome}"
    );

    let summary = json!({
        "status": "passed",
        "mode": "structural",
        "note": "every case here attacks proof structure rather than witness arithmetic. `per_shard_verifier_would_accept` records what the previous verification endpoint did, which is the bug these regressions pin.",
        "honest_control": {
            "execution_records": prepared.records.len(),
            "serialized_shards": shard_count,
            "proof_sha256": sha(&honest_bytes),
            "complete_verifier_accepted": true,
        },
        "cases": results,
        "total_seconds": started.elapsed().as_secs_f64(),
    });
    write_json(output.join("result.json"), &summary)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    println!("STRUCTURAL_PASSED");
    Ok(())
}

// ---------------------------------------------------------------------------
// Mode: continuity
//
// Two gaps the independent review identified in the structural coverage:
//
//  * the nonzero-exit regression mutated a *non-execution* shard's public
//    values, so an exit-chain structural check rejected it and the
//    successful-exit policy was never exercised on a genuinely proved failing
//    run; and
//  * the guest regression had only one advancing CPU shard, so no test
//    attacked a transition between two execution shards.
//
// Both are covered here with real proofs.
// ---------------------------------------------------------------------------

/// Scheduling options that force several execution shards. Only the minimal
/// trace chunk threshold changes; the FRI parameters, stacking height and
/// maximum log row count are the suite's defaults throughout.
fn multishard_opts() -> SP1CoreOpts {
    let mut opts = core_opts();
    // `minimal_trace_chunk_threshold` bounds a chunk by **memory entries**, not
    // by cycles, which is why register-only padding never splits a chunk however
    // many cycles it burns. Lowering it, and driving the fixture with memory
    // traffic, is what produces several advancing execution shards.
    //
    // The area and height thresholds are deliberately left at their defaults.
    // Lowering either drives the per-syscall deferral threshold for some stock
    // syscall to zero, and the record splitter then panics on a zero chunk size
    // even for syscalls the program never calls.
    //
    // FRI parameters, stacking height and maximum log row count are the suite's
    // defaults throughout.
    opts.minimal_trace_chunk_threshold = 1 << 11;
    opts
}

fn count_execution_shards(proof: &Proof) -> usize {
    proof
        .shard_proofs
        .iter()
        .filter(|sh| pv_of(sh).is_execution_shard == SP1Field::one())
        .count()
}

async fn prove_program(
    program: Arc<Program>,
    opts: SP1CoreOpts,
) -> Result<(
    Proof,
    MachineVerifyingKey<SP1GlobalContext>,
    SimpleProver<SP1GlobalContext, InnerSC<Air>, Cpu>,
    usize,
    u64,
)> {
    let (records, cycles) =
        generate_records::<SP1Field>(program.clone(), SP1Stdin::new(), opts, [0; 4])?;
    let verifier = ShardVerifier::from_basefold_parameters(
        FriConfig::default_fri_config(),
        LOG_STACKING_HEIGHT,
        MAX_LOG_ROW_COUNT,
        Air::machine(),
    );
    let cpu = Cpu::new(verifier.clone());
    let prover = SimpleProver::new(verifier, cpu.clone());
    let (pk, vk) = prover.setup(program).await;
    let pk = unsafe { pk.into_inner() };
    let mut shards = Vec::new();
    for record in &records {
        let data = cpu
            .trace_generator()
            .generate_main_traces(record.clone(), MAX_LOG_ROW_COUNT, ProverSemaphore::new(1))
            .await;
        let mut challenger = SP1GlobalContext::default_challenger();
        pk.vk.observe_into(&mut challenger);
        let (proof, permit) = cpu.prove_shard_with_data(
            ShardData {
                pk: pk.clone(),
                main_trace_data: data,
            },
            challenger,
        );
        drop(permit);
        shards.push(proof);
    }
    let generated = shards.len();
    ensure!(generated == records.len(), "shard assembly lost shards");
    let proof = MachineProof {
        shard_proofs: canonical_shard_order(shards),
    };
    let vk_bytes = bincode::serialize(&vk)?;
    let restored: MachineVerifyingKey<SP1GlobalContext> = bincode::deserialize(&vk_bytes)?;
    Ok((proof, restored, prover, records.len(), cycles))
}

async fn run_continuity(output: &Path) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    fs::create_dir_all(output)?;
    let started = Instant::now();
    let add_op = *OPS.iter().find(|o| o.id == "vesta-add").unwrap();
    let pts = multiples(Curve::Vesta, 4);
    let mut results = Vec::new();

    // ---------------------------------------------------------------
    // 1. A genuinely proved nonzero-exit run.
    //
    // The program is a complete, structurally valid execution that halts with
    // exit code 1. The canonical checks must therefore all pass, and only the
    // application policy may reject it. That is the regression the previous
    // nonzero-exit case could not provide, because it tampered with a
    // non-execution shard and tripped the exit chain instead.
    // ---------------------------------------------------------------
    let exit_calls = vec![make_call(add_op, pts[0].clone(), Some(pts[1].clone()))?];
    let (exit_program, exit_fixture) =
        build_program_at(&exit_calls, FIXTURE_BASE, true, false, 0, Some(0), 0, 1)?;
    let (exit_proof, exit_vk, exit_prover, exit_records, exit_cycles) =
        prove_program(exit_program, core_opts()).await?;
    let structure_only = check_execution_structure(&exit_vk, &exit_proof);
    let crypto_only = exit_prover.verify(&exit_vk, &exit_proof);
    let with_policy = verify_complete_execution(
        &exit_prover,
        &exit_vk,
        &exit_proof,
        &Policy {
            require_successful_exit: true,
            expected_committed_digest: None,
        },
    );
    let without_policy =
        verify_complete_execution(&exit_prover, &exit_vk, &exit_proof, &Policy::default());
    let exit_code = pv_of(exit_proof.shard_proofs.last().unwrap())
        .exit_code
        .as_canonical_u32();
    fs::write(
        output.join("nonzero-exit.proof.bin"),
        bincode::serialize(&exit_proof)?,
    )?;
    println!(
        "CONTINUITY proved-nonzero-exit exit_code={exit_code} canonical_ok={} policy_rejects={} check={:?}",
        structure_only.is_ok(),
        with_policy.is_err(),
        with_policy.as_ref().err().map(|e| e.check)
    );
    ensure!(
        exit_code == 1,
        "the fixture did not halt with exit code 1, got {exit_code}"
    );
    ensure!(
        structure_only.as_ref().err().map(|e| e.check) == Some("canonical_successful_exit"),
        "SP1 6.10 canonical verification must reject a nonzero exit: {:?}",
        structure_only.as_ref().err()
    );
    ensure!(
        crypto_only.is_ok(),
        "per-shard cryptography must accept a valid failing run"
    );
    ensure!(
        without_policy.as_ref().err().map(|e| e.check) == Some("canonical_successful_exit"),
        "SP1 6.10 must reject a nonzero exit even without application policy: {:?}",
        without_policy.as_ref().err()
    );
    ensure!(
        with_policy.as_ref().err().map(|e| e.check) == Some("canonical_successful_exit"),
        "the successful-exit policy must be what rejects, got {:?}",
        with_policy.as_ref().err().map(|e| e.check)
    );
    results.push(json!({
        "case": "proved-nonzero-exit",
        "intent": "a complete, structurally valid execution that halts with exit code 1. The bare per-shard verifier accepts it; SP1 6.10 canonical verification rejects before application policy",
        "halt_exit_code": exit_fixture["halt_exit_code"],
        "observed_exit_code": exit_code,
        "execution_records": exit_records,
        "guest_cycles": exit_cycles,
        "canonical_structure_accepted": structure_only.is_ok(),
        "per_shard_cryptography_accepted": crypto_only.is_ok(),
        "complete_verifier_without_policy_accepted": without_policy.is_ok(),
        "complete_verifier_with_policy_rejected": with_policy.is_err(),
        "rejection_check": with_policy.as_ref().err().map(|e| e.check),
        "expected_rejection_check": "canonical_successful_exit",
        "rejecting_layer": "canonical",
        "passed": true,
    }));

    // ---------------------------------------------------------------
    // 2. A fixture with several advancing CPU shards, then attacks on the
    //    transitions between them.
    // ---------------------------------------------------------------
    // Each call moves 16 operand words plus 16 more for its output comparison,
    // so the call count is what crosses the chunk's memory-entry budget.
    let mut chosen_calls = 0usize;
    let mut probe = Vec::new();
    let build_multi = |count: usize| -> Result<(Arc<Program>, Value)> {
        let mut calls = Vec::with_capacity(count);
        for i in 0..count {
            let a = i % 3;
            calls.push(make_call(add_op, pts[a].clone(), Some(pts[a + 1].clone()))?);
        }
        build_program_at(&calls, FIXTURE_BASE, true, false, 0, Some(0), 0, 0)
    };
    for count in [32usize, 64, 128, 256] {
        let (candidate, _) = build_multi(count)?;
        let (records, cycles) =
            generate_records::<SP1Field>(candidate, SP1Stdin::new(), multishard_opts(), [0; 4])?;
        let advancing = records
            .iter()
            .filter(|r| r.public_values.is_execution_shard == 1)
            .count();
        probe.push(json!({"calls": count, "guest_cycles": cycles,
                          "execution_records": records.len(),
                          "advancing_execution_shards": advancing}));
        println!(
            "CONTINUITY probe calls={count} cycles={cycles} records={} advancing={advancing}",
            records.len()
        );
        if advancing >= 2 {
            chosen_calls = count;
            break;
        }
    }
    ensure!(
        chosen_calls > 0,
        "no probed fixture size produced two advancing execution shards: {}",
        serde_json::to_string(&probe)?
    );
    let (multi_program, _) = build_multi(chosen_calls)?;
    let (multi_proof, multi_vk, multi_prover, multi_records, multi_cycles) =
        prove_program(multi_program, multishard_opts()).await?;
    let exec_shards = count_execution_shards(&multi_proof);
    let policy = Policy {
        require_successful_exit: true,
        expected_committed_digest: None,
    };
    let honest = verify_complete_execution(&multi_prover, &multi_vk, &multi_proof, &policy);
    println!(
        "CONTINUITY multi-cpu-shard records={multi_records} shards={} execution_shards={exec_shards} cycles={multi_cycles} honest_accepted={}",
        multi_proof.shard_proofs.len(),
        honest.is_ok()
    );
    ensure!(
        exec_shards >= 2,
        "the continuity fixture must advance across at least two CPU shards, got {exec_shards}"
    );
    ensure!(
        honest.is_ok(),
        "the multi-shard honest control must verify: {:?}",
        honest.err()
    );
    fs::write(
        output.join("multi-shard.proof.bin"),
        bincode::serialize(&multi_proof)?,
    )?;

    // Indices of the advancing execution shards, in canonical order.
    let exec_idx: Vec<usize> = multi_proof
        .shard_proofs
        .iter()
        .enumerate()
        .filter(|(_, sh)| pv_of(sh).is_execution_shard == SP1Field::one())
        .map(|(i, _)| i)
        .collect();

    let mut attacks: Vec<(&str, &str, &str, Proof)> = Vec::new();
    {
        let mut v = multi_proof.shard_proofs.clone();
        v.remove(exec_idx[0]);
        attacks.push((
            "drop-first-execution-shard",
            "the first advancing CPU shard removed. That shard carries the is_first_execution_shard flag, so the uniqueness check fires before the timestamp chain does",
            "first_execution_shard_present",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = multi_proof.shard_proofs.clone();
        v.remove(exec_idx[1]);
        attacks.push((
            "drop-second-execution-shard",
            "the second advancing CPU shard removed",
            "timestamp_chain",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = multi_proof.shard_proofs.clone();
        v.swap(exec_idx[0], exec_idx[1]);
        attacks.push((
            "swap-execution-shards",
            "the two advancing CPU shards exchanged, so each starts from the other's state",
            "timestamp_chain",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = multi_proof.shard_proofs.clone();
        v.insert(exec_idx[1], multi_proof.shard_proofs[exec_idx[0]].clone());
        attacks.push((
            "duplicate-first-execution-shard",
            "the first advancing CPU shard replayed before the second. Its is_first_execution_shard flag is now set on two shards, so the uniqueness check fires before the timestamp chain",
            "first_execution_shard_unique",
            MachineProof { shard_proofs: v },
        ));
    }
    {
        let mut v = multi_proof.shard_proofs.clone();
        let target = exec_idx[1];
        tamper_pv(&mut v[target], |pv| {
            pv.initial_timestamp[3] = pv.initial_timestamp[3] + SP1Field::one();
        });
        attacks.push((
            "forge-execution-shard-transition",
            "the second CPU shard's initial timestamp advanced by one, forging the handover",
            "timestamp_chain",
            MachineProof { shard_proofs: v },
        ));
    }

    for (id, intent, expected_check, candidate) in attacks {
        let outcome = verify_complete_execution(&multi_prover, &multi_vk, &candidate, &policy);
        let per_shard = multi_prover.verify(&multi_vk, &candidate).is_ok();
        let check = outcome.as_ref().err().map(|e| e.check);
        println!(
            "CONTINUITY {id} rejected={} check={} expected={expected_check} per_shard_accepted={per_shard}",
            outcome.is_err(),
            check.unwrap_or("-")
        );
        ensure!(
            outcome.is_err(),
            "{id}: the complete verifier accepted a transition attack"
        );
        ensure!(
            check == Some(expected_check),
            "{id}: expected rejection at {expected_check}, got {check:?}"
        );
        results.push(json!({
            "case": id,
            "intent": intent,
            "rejecting_layer": "structural",
            "shards": candidate.shard_proofs.len(),
            "complete_verifier_rejected": true,
            "rejection_check": check,
            "expected_rejection_check": expected_check,
            "per_shard_verifier_would_accept": per_shard,
            "passed": true,
        }));
    }

    let summary = json!({
        "status": "passed",
        "mode": "continuity",
        "note": "covers the two gaps the independent review found in structural coverage: the successful-exit policy had no genuinely proved failing run to reject, and no fixture advanced across more than one CPU shard",
        "multi_shard_fixture": {
            "execution_records": multi_records,
            "serialized_shards": multi_proof.shard_proofs.len(),
            "advancing_execution_shards": exec_shards,
            "guest_cycles": multi_cycles,
            "pasta_calls": chosen_calls,
            "size_probe": probe,
            "scheduling_change": "sharding_threshold.element_threshold lowered to 2^22 for this fixture only, because the execution-shard split is driven by trace area rather than cycle count; the height threshold is deliberately left alone, since lowering it drives the per-syscall deferral threshold for the 1,599-column Pasta chips to zero. FRI parameters, stacking height and maximum log row count are the suite defaults",
        },
        "cases": results,
        "total_seconds": started.elapsed().as_secs_f64(),
    });
    write_json(output.join("result.json"), &summary)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    println!("CONTINUITY_PASSED");
    Ok(())
}

/// Derive the SP1 program verification key for an arbitrary ELF under *this*
/// machine, and print its `bytes32` hash.
///
/// This exists to answer one question: does adding the four Pasta chips change
/// the verification key of a program that never calls them? The verification
/// key is `{pc_start, initial_global_cumulative_sum, preprocessed_commit,
/// enable_untrusted_programs}`, and the commitment covers only chips that
/// actually carry a preprocessed trace. The Pasta chips carry none, so the
/// prediction is that the key is unchanged — testable against the stock-SP1
/// value recorded by the RunPod r4 run.
async fn run_vk(elf_path: &Path) -> Result<()> {
    let elf = fs::read(elf_path).context("read ELF")?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);
    // SP1's production core parameters, so the key is comparable with one
    // produced by the stock prover. The suite's own 17/17 is sized for tiny
    // fixtures and cannot hold a real program's preprocessed trace.
    const CORE_LOG_STACKING_HEIGHT: u32 = 21;
    const CORE_MAX_LOG_ROW_COUNT: usize = 22;
    let verifier = ShardVerifier::from_basefold_parameters(
        FriConfig::default_fri_config(),
        CORE_LOG_STACKING_HEIGHT,
        CORE_MAX_LOG_ROW_COUNT,
        Air::machine(),
    );
    let cpu = Cpu::new(verifier.clone());
    let prover = SimpleProver::new(verifier, cpu);
    let (_, vk) = prover.setup(program).await;
    let report = json!({
        "elf_path": elf_path.to_string_lossy(),
        "elf_sha256": sha(&elf),
        "machine_chips": Air::machine().chips().len(),
        "log_stacking_height": CORE_LOG_STACKING_HEIGHT,
        "max_log_row_count": CORE_MAX_LOG_ROW_COUNT,
        "vk_bytes32": sp1_hypercube::HashableKey::bytes32(&vk),
        "vk_serialized_sha256": sha(&bincode::serialize(&vk)?),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Dump a saved proof's per-shard public-value chain and chip inventory.
/// Used both for diagnosis and to enforce the compiled fixture's Pasta-chip
/// inventory requirement.
fn inspect_proof(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).context("read proof")?;
    let proof: Proof = bincode::deserialize(&bytes).context("deserialize proof")?;
    let limbs3 = |v: &[SP1Field; 3]| v.iter().map(|f| f.as_canonical_u32()).collect::<Vec<_>>();
    let limbs4 = |v: &[SP1Field; 4]| v.iter().map(|f| f.as_canonical_u32()).collect::<Vec<_>>();
    let mut shards = Vec::new();
    let mut all_chips: BTreeMap<String, usize> = BTreeMap::new();
    for (i, shard) in proof.shard_proofs.iter().enumerate() {
        let pv = pv_of(shard);
        let chips: Vec<String> = shard.opened_values.chips.keys().cloned().collect();
        for c in &chips {
            *all_chips.entry(c.clone()).or_default() += 1;
        }
        shards.push(json!({
            "index": i,
            "is_first_execution_shard": pv.is_first_execution_shard.as_canonical_u32(),
            "is_execution_shard": pv.is_execution_shard.as_canonical_u32(),
            "initial_timestamp": limbs4(&pv.initial_timestamp),
            "last_timestamp": limbs4(&pv.last_timestamp),
            "pc_start": limbs3(&pv.pc_start),
            "next_pc": limbs3(&pv.next_pc),
            "prev_exit_code": pv.prev_exit_code.as_canonical_u32(),
            "exit_code": pv.exit_code.as_canonical_u32(),
            "previous_init_addr": limbs3(&pv.previous_init_addr),
            "last_init_addr": limbs3(&pv.last_init_addr),
            "previous_finalize_addr": limbs3(&pv.previous_finalize_addr),
            "last_finalize_addr": limbs3(&pv.last_finalize_addr),
            "commit_syscall": pv.commit_syscall.as_canonical_u32(),
            "commit_deferred_syscall": pv.commit_deferred_syscall.as_canonical_u32(),
            "committed_digest_hex": digest_bytes(pv)
                .map(hex::encode)
                .unwrap_or_else(|e| format!("invalid: {e}")),
            "chips": chips,
        }));
    }
    let pasta: Vec<&str> = OPS
        .iter()
        .filter(|o| all_chips.contains_key(o.chip))
        .map(|o| o.chip)
        .collect();
    Ok(json!({
        "proof_path": path.to_string_lossy(),
        "proof_sha256": sha(&bytes),
        "serialized_shards": proof.shard_proofs.len(),
        "chip_shard_counts": all_chips,
        "pasta_chips_present": pasta,
        "shards": shards,
    }))
}

async fn prove_case(
    cpu: &Cpu,
    pk: Arc<Key>,
    records: &[ExecutionRecord],
    op: &Op,
    case: Case,
) -> Result<(Proof, Vec<Value>)> {
    // Execution order is the canonical order, and every generated shard proof is
    // retained. The previous BTreeMap keyed on
    // (initial_timestamp, last_timestamp, previous_init_addr, previous_finalize_addr)
    // silently overwrote distinct precompile shards that share that tuple.
    let mut shards: Vec<_> = Vec::new();
    let mut mutations = Vec::new();
    for original in records {
        let mut record = original.clone();
        if case != Case::Honest && !case.is_trace_mutation() {
            if let Some(mutation) = mutate_record(&mut record, op, case)? {
                mutations.push(mutation);
            }
        }
        let mut data = cpu
            .trace_generator()
            .generate_main_traces(record, MAX_LOG_ROW_COUNT, ProverSemaphore::new(1))
            .await;
        if case.is_trace_mutation() {
            if let Some(mutation) = mutate_trace(&mut data, op, case)? {
                mutations.push(mutation);
            }
        }
        let mut challenger = SP1GlobalContext::default_challenger();
        pk.vk.observe_into(&mut challenger);
        let (proof, permit) = cpu.prove_shard_with_data(
            ShardData {
                pk: pk.clone(),
                main_trace_data: data,
            },
            challenger,
        );
        drop(permit);
        shards.push(proof);
    }
    ensure!(
        shards.len() == records.len(),
        "shard assembly lost shards: {} generated from {} execution records",
        shards.len(),
        records.len()
    );
    ensure!(
        mutations.len() == usize::from(case != Case::Honest),
        "expected exactly {} mutation(s) for {:?}, applied {}",
        usize::from(case != Case::Honest),
        case,
        mutations.len()
    );
    let generated = shards.len();
    let shard_proofs = canonical_shard_order(shards);
    ensure!(
        shard_proofs.len() == generated && generated == records.len(),
        "canonical ordering changed the shard count: {} records, {generated} generated, {} ordered",
        records.len(),
        shard_proofs.len()
    );
    Ok((MachineProof { shard_proofs }, mutations))
}

struct Prepared {
    records: Vec<ExecutionRecord>,
    cycles: u64,
    cpu: Cpu,
    prover: SimpleProver<SP1GlobalContext, InnerSC<Air>, Cpu>,
    pk: Arc<Key>,
    vk_bytes: Vec<u8>,
    restored_vk: MachineVerifyingKey<SP1GlobalContext>,
    program_bytes: Vec<u8>,
    trace_seconds: f64,
    setup_seconds: f64,
}

fn core_opts() -> SP1CoreOpts {
    let mut opts = SP1CoreOpts::default();
    opts.minimal_trace_chunk_threshold = 1 << 16;
    opts.trace_chunk_slots = 0;
    opts.memory_limit = 128 * 1024 * 1024;
    opts.shard_size = 1 << 10;
    apply_retention_env(&mut opts);
    opts
}

/// `PASTA_RETAIN` selects the precompile memory-retention routing for the CPU
/// suites: `uint` keeps `UINT256_MUL` events in CPU shards, `pasta-uint` also
/// both Pasta additions (the r107 "both-all" routing). Unset keeps the default.
fn apply_retention_env(opts: &mut SP1CoreOpts) {
    use sp1_core_executor::RetainedEventsPreset;
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

/// Options for proving a real program, as opposed to the tiny fixtures
/// `core_opts` is tuned for. This mirrors `sp1_gpu_prover::local_gpu_opts`,
/// which sets a 2^24 shard size and lowers the element threshold on cards with
/// 30 GB or less, but without its panic on small GPUs.
#[cfg(feature = "cuda")]
fn core_opts_production() -> SP1CoreOpts {
    let mut opts = SP1CoreOpts::default();
    opts.shard_size = 1 << 24;
    if let Ok((_, total)) = sp1_gpu_cudart::cuda_memory_info() {
        let gb = (total as f64) / (1024.0 * 1024.0 * 1024.0);
        if gb.ceil() as usize + 4 <= 30 {
            opts.sharding_threshold.element_threshold =
                sp1_core_executor::ELEMENT_THRESHOLD - (1 << 26) - (1 << 25);
        }
    }
    opts
}

async fn prepare(program: Arc<Program>) -> Result<Prepared> {
    let program_bytes = bincode::serialize(&*program)?;
    let trace_start = Instant::now();
    let (records, cycles) =
        generate_records::<SP1Field>(program.clone(), SP1Stdin::new(), core_opts(), [0; 4])?;
    let trace_seconds = trace_start.elapsed().as_secs_f64();
    let verifier = ShardVerifier::from_basefold_parameters(
        FriConfig::default_fri_config(),
        LOG_STACKING_HEIGHT,
        MAX_LOG_ROW_COUNT,
        Air::machine(),
    );
    let cpu = Cpu::new(verifier.clone());
    let prover = SimpleProver::new(verifier, cpu.clone());
    let setup_start = Instant::now();
    let (pk, vk) = prover.setup(program).await;
    let pk = unsafe { pk.into_inner() };
    let setup_seconds = setup_start.elapsed().as_secs_f64();
    let vk_bytes = bincode::serialize(&vk)?;
    let restored_vk: MachineVerifyingKey<SP1GlobalContext> = bincode::deserialize(&vk_bytes)?;
    Ok(Prepared {
        records,
        cycles,
        cpu,
        prover,
        pk,
        vk_bytes,
        restored_vk,
        program_bytes,
        trace_seconds,
        setup_seconds,
    })
}

/// Assert the registered trace cost of every new AIR equals its actual column
/// count, and that every AIR ID has a cost entry at all.
/// Hash each operation's generated witness trace and its byte/range
/// dependencies.
///
/// This exists to settle one specific question: whether a change to how the
/// *host* computes a witness leaves the committed trace bit-identical. The
/// proof hash cannot answer it, because proving is randomized — two runs of
/// `--mode guest` over the same inputs produce different proof bytes while
/// sharing a verification key. Constraint checks cannot answer it either: a
/// different but still valid witness would satisfy them. So hash the trace.
/// Execute the r5 Mina candidate on the CPU and report cycles, optionally
/// timing the Pasta chips' witness generation on the real workload.
///
/// No proving and no GPU. Records are streamed, so peak memory does not grow
/// with the program — which matters here, because this machine has 16 GB and
/// the r11 GPU run peaked at 9 GiB of host RSS.
///
/// Two things this answers that fixtures cannot. First, whether a host-side
/// witness change moves the cycle count: it must not, because cycles come from
/// the executor, which never calls `populate`. Second, what the Pasta chips'
/// witness generation actually costs across 2,002,975 real additions rather
/// than across a 1024-call fixture.
/// Peak resident set size of this process, in bytes.
///
/// `ru_maxrss` is bytes on macOS and kilobytes on Linux.
fn peak_rss_bytes() -> u64 {
    let mut usage: libc::rusage = unsafe { core::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0;
    }
    let raw = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    }
}

/// Compatibility entry point for the dedicated CPU-only accounting library.
/// Version 3 requires pinned execution expectations and rejects capped, failed
/// or output-mismatched runs. It never dispatches a proving operation.
async fn run_trace_accounting(
    elf_path: &Path,
    input_dir: &Path,
    output: &Path,
    max_records: Option<usize>,
    reference: &Path,
) -> Result<()> {
    pasta_precompile_proof::trace_accounting::run(
        elf_path,
        input_dir,
        output,
        max_records,
        reference,
    )
}

async fn run_r5_cycles(
    elf_path: &Path,
    input_dir: &Path,
    output: &Path,
    time_witness: bool,
    max_records: Option<usize>,
) -> Result<()> {
    let started = Instant::now();
    let elf = fs::read(elf_path).context("read the r5 ELF")?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);

    let mut stdin = SP1Stdin::new();
    stdin.write_slice(&fs::read(input_dir.join("proof-input.bin"))?);
    stdin.write_slice(&fs::read(input_dir.join("receipt-input.bin"))?);

    // `SP1CoreOpts::default()` already reads SHARD_SIZE, ELEMENT_THRESHOLD,
    // HEIGHT_THRESHOLD, MINIMAL_TRACE_CHUNK_THRESHOLD, TRACE_CHUNK_SLOTS and
    // MEMORY_LIMIT from the environment, so nothing is hardcoded here. That
    // matters on a small host: the defaults target a 2 GiB trace chunk, a
    // 24 GiB memory limit and a 4,194,304-row height threshold, and at width
    // 1599 that last one alone implies a 26 GB chip trace.
    let mut opts = SP1CoreOpts::default();
    opts.global_dependencies_opt = std::env::var("PASTA_GLOBAL_DEPS_OPT")
        .map(|v| v != "0")
        .unwrap_or(false);
    // Captured before `opts` moves into the streaming call.
    let global_deps_opt = opts.global_dependencies_opt;
    let shard_size = opts.shard_size;
    let element_threshold = opts.sharding_threshold.element_threshold;
    let height_threshold = opts.sharding_threshold.height_threshold;
    let chunk_threshold = opts.minimal_trace_chunk_threshold;
    let memory_limit = opts.memory_limit;

    let mut records_seen = 0usize;
    let mut per_op: BTreeMap<&str, usize> = OPS.iter().map(|o| (o.id, 0)).collect();
    // UINT256_MUL is more numerous than the additions (3,443,579 against
    // 2,002,975) and has never been attributed. Counting it per record gives
    // its distribution over execution time, and since the r5 stages run
    // sequentially — setup, then verify containing the three pickles stages —
    // that distribution localises the calls to a stage without touching the
    // executor.
    let uint_codes = [
        ("uint256-mul", SyscallCode::UINT256_MUL),
        ("uint256-add-carry", SyscallCode::UINT256_ADD_CARRY),
        ("uint256-mul-carry", SyscallCode::UINT256_MUL_CARRY),
    ];
    let mut per_uint: BTreeMap<&str, usize> = uint_codes.iter().map(|(n, _)| (*n, 0)).collect();
    let mut timeline: Vec<(usize, usize, usize)> = Vec::new();
    let mut witness_seconds: BTreeMap<&str, f64> = OPS.iter().map(|o| (o.id, 0.0)).collect();
    // Dependency generation populates every row a second time, so it is timed
    // separately from trace generation rather than folded into it.
    let mut dependency_seconds: BTreeMap<&str, f64> =
        OPS.iter().map(|o| (o.id, 0.0)).collect();
    // Opt-in whole-workload witness digest. Equal digests across two builds
    // mean every Pasta row and every byte lookup it emits is bit-identical for
    // the real workload, not just for the one-event fixtures of
    // `witness-digest`.
    let mut digests: Option<BTreeMap<&str, WitnessDigest>> =
        (time_witness && std::env::var_os("PASTA_WITNESS_DIGEST").is_some())
            .then(|| OPS.iter().map(|o| (o.id, WitnessDigest::default())).collect());
    let mut stopped_early = false;

    // Opt-in digest of every streamed record, in order, for byte-level
    // comparison of two record producers on the same workload.
    let mut record_digest =
        std::env::var_os("PASTA_RECORD_DIGEST").is_some().then(Sha256::new);
    let exec_start = Instant::now();
    let summary = sp1_core_machine::utils::generate_records_streaming_with_summary::<SP1Field, _>(
        program,
        stdin,
        opts,
        [0; 4],
        |record| {
            let mut record = record;
            if let Some(hasher) = record_digest.as_mut() {
                hash_record_canonical(hasher, &mut record);
            }
            records_seen += 1;
            let mut rec_uint = 0usize;
            for (name, code) in &uint_codes {
                let n = record.get_precompile_events(*code).len();
                *per_uint.get_mut(*name).unwrap() += n;
                rec_uint += n;
            }
            let mut rec_add = 0usize;
            for op in &OPS {
                let n = record.get_precompile_events(op.syscall).len();
                if n == 0 {
                    continue;
                }
                if op.kind == Kind::Add {
                    rec_add += n;
                }
                *per_op.get_mut(op.id).unwrap() += n;
                if time_witness {
                    let digest = digests.as_mut().map(|d| d.get_mut(op.id).unwrap());
                    let (deps_s, trace_s) = time_chip_witness(&record, op, digest);
                    *witness_seconds.get_mut(op.id).unwrap() += trace_s;
                    *dependency_seconds.get_mut(op.id).unwrap() += deps_s;
                }
            }
            timeline.push((records_seen, rec_add, rec_uint));
            if let Some(cap) = max_records {
                if records_seen >= cap {
                    stopped_early = true;
                    return false;
                }
            }
            true
        },
    )
    .map_err(|e| anyhow::anyhow!("execution failed: {e}"))?;
    let streaming_seconds = exec_start.elapsed().as_secs_f64();
    let cycles = summary.cycles;
    let producer_profile = pipeline::producer_profile_json(&summary.profile);
    let record_digest = record_digest.map(|h| hex::encode(h.finalize()));

    let total_adds = per_op["vesta-add"] + per_op["pallas-add"];
    let total_doubles = per_op["vesta-double"] + per_op["pallas-double"];
    let trace_total: f64 = witness_seconds.values().sum();
    let dependency_total: f64 = dependency_seconds.values().sum();
    let witness_total = trace_total + dependency_total;
    let witness_digests = digests.map(|d| {
        d.into_iter()
            .map(|(id, digest)| (id, digest.finish()))
            .collect::<BTreeMap<_, _>>()
    });
    // The witness timing happens inside the streaming callback, so subtract it
    // to get the cost of record generation alone: execution, splicing, tracing
    // and dependency generation. These two are disjoint and sum to the total,
    // which is the separation r11's overlapping spans could not give.
    let record_generation_seconds = streaming_seconds - witness_total;

    // 50 equal bins over the record stream: additions and uint256 calls per
    // bin, so the two can be compared phase by phase.
    let timeline_bins = {
        let bins = 50usize;
        let n = timeline.len().max(1);
        let mut out = Vec::new();
        for b in 0..bins {
            let lo = b * n / bins;
            let hi = (((b + 1) * n / bins).max(lo + 1)).min(n);
            if lo >= hi {
                continue;
            }
            let (mut a, mut u) = (0usize, 0usize);
            for (_, ra, ru) in &timeline[lo..hi] {
                a += ra;
                u += ru;
            }
            out.push(json!({"bin": b, "records": hi - lo, "additions": a, "uint256": u}));
        }
        out
    };

    fs::create_dir_all(output)?;
    let report = json!({
        "mode": "r5-cycles",
        "candidate": "r5-both-v16-p13",
        "proved": false,
        "gpu_used": false,
        "elf_sha256": sha(&elf),
        "guest_cycles": cycles,
        "expected_cycles": 621_941_587u64,
        "cycles_match_recorded_run": cycles == 621_941_587 && !stopped_early,
        "stopped_early": stopped_early,
        "records_streamed": records_seen,
        "precompile_events": per_op,
        "uint256_events": per_uint,
        "uint256_total": per_uint.values().sum::<usize>(),
        "timeline_bins": timeline_bins,
        "total_additions": total_adds,
        "total_doublings": total_doubles,
        "expected_total_additions": 2_002_975usize,
        "additions_match_recorded_run": total_adds == 2_002_975 && !stopped_early,
        "global_dependencies_opt": global_deps_opt,
        "shard_size": shard_size,
        "streaming_total_seconds": streaming_seconds,
        "record_generation_seconds": record_generation_seconds,
        "phase_split_note": "record_generation_seconds and pasta_witness_total_seconds are disjoint and sum to streaming_total_seconds",
        "opts_used": {
            "shard_size": shard_size,
            "element_threshold": element_threshold,
            "height_threshold": height_threshold,
            "minimal_trace_chunk_threshold": chunk_threshold,
            "memory_limit": memory_limit,
        },
        "pasta_witness_generation_seconds": if time_witness { json!(witness_seconds) } else { json!(null) },
        "pasta_witness_dependency_seconds": if time_witness { json!(dependency_seconds) } else { json!(null) },
        "pasta_witness_trace_total_seconds": if time_witness { json!(trace_total) } else { json!(null) },
        "pasta_witness_dependency_total_seconds": if time_witness { json!(dependency_total) } else { json!(null) },
        "pasta_witness_total_seconds": if time_witness { json!(witness_total) } else { json!(null) },
        "pasta_witness_digests": json!(witness_digests),
        "record_stream_sha256": record_digest,
        "producer_profile": producer_profile,
        "rayon_num_threads": std::env::var("RAYON_NUM_THREADS").ok(),
        "pasta_witness_us_per_addition": if time_witness && total_adds > 0 {
            json!(witness_total * 1e6 / total_adds as f64)
        } else { json!(null) },
        "total_seconds": started.elapsed().as_secs_f64(),
    });
    write_json(output.join("result.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn witness_digest() -> Result<Value> {
    let mut operations = BTreeMap::new();
    for op in &OPS {
        let pts = multiples(op.curve, 3);
        let q = if op.kind == Kind::Add {
            Some(&pts[1])
        } else {
            None
        };
        let (program, _) = build_raw_program(op, &pts[0], q)?;
        let (records, _) =
            generate_records::<SP1Field>(program, SP1Stdin::new(), core_opts(), [0; 4])?;
        let mut found = None;
        for record in records.iter() {
            if record.get_precompile_events(op.syscall).is_empty() {
                continue;
            }
            let (trace, deps) = chip_trace_and_deps(record, op)?;
            let deps_bytes = bincode::serialize(&deps)?;
            found = Some(json!({
                "trace_sha256": sha(&trace),
                "trace_bytes": trace.len(),
                "dependencies_sha256": sha(&deps_bytes),
                "dependencies_bytes": deps_bytes.len(),
            }));
            break;
        }
        operations.insert(
            op.id,
            found.with_context(|| format!("no record carried a {} event", op.id))?,
        );
    }
    Ok(json!({
        "mode": "witness-digest",
        "what_this_proves": "equal digests across two builds mean the committed witness trace and the byte/range events it emits are bit-identical, so any difference is confined to host-side computation",
        "operations": operations,
    }))
}

// ---------------------------------------------------------------------------
// Mode: pasta-fp-check
//
// The fixed-modulus Pasta field syscalls (PALLAS_FP_* / VESTA_FP_*, served by
// SP1's FpOpChip instantiated at the Pasta base fields). A raw program runs
// add, sub and mul on random and edge operands in both fields; each result is
// compared on the guest against an integer reference baked into the memory
// image. Then: executor results against the reference, honest rows against
// the AIR, a tampered result limb against the AIR, a sweep of every column,
// and a CPU core proof of the program checked by the complete verifier.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FpCall {
    code: SyscallCode,
    x: BigUint,
    y: BigUint,
    expected: BigUint,
}

fn fp_words(value: &BigUint) -> [u64; 4] {
    let mut out = [0u64; 4];
    let digits = value.to_u64_digits();
    out[..digits.len()].copy_from_slice(&digits);
    out
}

fn build_fp_program(calls: &[FpCall], tag: u32) -> Result<Arc<Program>> {
    // One 4 KiB-aligned slot per call keeps every operand address's low 12
    // bits below 0x800, as `load_address` requires.
    const BASE: u64 = 0x10_0000;
    const STRIDE: u64 = 0x1000;
    let mut instructions = Vec::new();
    let mut branches = Vec::new();
    let mut image: Vec<(u64, [u64; 4])> = Vec::new();
    for (i, call) in calls.iter().enumerate() {
        let base = BASE + i as u64 * STRIDE;
        let (x, y, expected) = (base, base + 0x100, base + 0x200);
        image.push((x, fp_words(&call.x)));
        image.push((y, fp_words(&call.y)));
        image.push((expected, fp_words(&call.expected)));
        load_address(&mut instructions, 10, x);
        load_address(&mut instructions, 11, y);
        let code = call.code as u32 as u64;
        instructions.push(Instruction::new(Opcode::LUI, 5, code & !0xfff, code & !0xfff, true, true));
        instructions.push(Instruction::new(Opcode::ADDI, 5, 5, code & 0xfff, false, true));
        instructions.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
        load_address(&mut instructions, 12, expected);
        for limb in 0..4u64 {
            instructions.push(Instruction::new(Opcode::LD, 20, 10, 8 * limb, false, true));
            instructions.push(Instruction::new(Opcode::LD, 21, 12, 8 * limb, false, true));
            branches.push(instructions.len());
            instructions.push(Instruction::new(Opcode::BNE, 20, 21, 0, false, true));
        }
    }
    add_commitments(&mut instructions, tag);
    add_halt(&mut instructions);
    let failure = instructions.len();
    instructions.push(Instruction::new(Opcode::UNIMP, 0, 0, 0, false, false));
    for i in branches {
        instructions[i].op_c = ((failure - i) * 4) as u64;
    }
    ensure!(0x2000 + 4 * instructions.len() as u64 <= BASE, "program overlaps its data");
    let mut program = Program::new(instructions, 0x2000, 0x2000);
    let memory = Arc::make_mut(&mut program.memory_image);
    for (address, words) in image {
        for (i, word) in words.into_iter().enumerate() {
            memory.insert(address + 8 * i as u64, word);
        }
    }
    Ok(Arc::new(program))
}

fn fp_calls(rng: &mut FuzzRng, per_op: usize) -> Vec<FpCall> {
    let fields = [
        (PallasBaseField::modulus(), [SyscallCode::PALLAS_FP_ADD, SyscallCode::PALLAS_FP_SUB, SyscallCode::PALLAS_FP_MUL]),
        (VestaBaseField::modulus(), [SyscallCode::VESTA_FP_ADD, SyscallCode::VESTA_FP_SUB, SyscallCode::VESTA_FP_MUL]),
    ];
    let mut calls = Vec::new();
    for (m, codes) in fields {
        let random = |rng: &mut FuzzRng| {
            let mut v = BigUint::zero();
            for _ in 0..4 {
                v = (v << 64) + BigUint::from(rng.next_u64());
            }
            v % &m
        };
        let mut operands = vec![
            (BigUint::zero(), BigUint::zero()),
            (BigUint::one(), &m - 1u32),
            (&m - 1u32, &m - 1u32),
            (&m - 1u32, BigUint::one()),
        ];
        while operands.len() < per_op {
            operands.push((random(rng), random(rng)));
        }
        for (i, code) in codes.into_iter().enumerate() {
            for (x, y) in &operands {
                let expected = match i {
                    0 => (x + y) % &m,
                    1 => (x + &m - y) % &m,
                    _ => (x * y) % &m,
                };
                calls.push(FpCall { code, x: x.clone(), y: y.clone(), expected });
            }
        }
    }
    calls
}

async fn run_pasta_fp_check(output: &Path) -> Result<()> {
    use sp1_core_machine::{
        operations::field::{field_op::FieldOpCols, range::FieldLtCols},
        syscall::precompiles::fptower::{num_fp_cols_supervisor, FpOpChip},
        SupervisorMode,
    };
    ensure!(!output.exists(), "use a fresh output directory");
    let mut rng = FuzzRng(fuzz_env_u64("PASTA_FUZZ_SEED", 0xf0_0001));
    let per_op = fuzz_env_u64("PASTA_FUZZ_ITERS", 24) as usize;
    let calls = fp_calls(&mut rng, per_op);
    let tag = 0xF0F0;
    let program = build_fp_program(&calls, tag)?;
    let (records, cycles) =
        generate_records::<SP1Field>(program.clone(), SP1Stdin::new(), core_opts(), [0; 4])?;

    let machine = Air::machine();
    let mut fields = Vec::new();
    for (name, key, modulus, width, arithmetic) in [
        (
            "PallasFpOpAssign",
            SyscallCode::PALLAS_FP_ADD,
            PallasBaseField::modulus(),
            num_fp_cols_supervisor::<PallasBaseField>(),
            core::mem::size_of::<FieldOpCols<u8, PallasBaseField>>()
                + core::mem::size_of::<FieldLtCols<u8, PallasBaseField>>(),
        ),
        (
            "VestaFpOpAssign",
            SyscallCode::VESTA_FP_ADD,
            VestaBaseField::modulus(),
            num_fp_cols_supervisor::<VestaBaseField>(),
            core::mem::size_of::<FieldOpCols<u8, VestaBaseField>>()
                + core::mem::size_of::<FieldLtCols<u8, VestaBaseField>>(),
        ),
    ] {
        let chip = machine.chips().iter().find(|c| c.name() == name).context("chip")?;
        let mut checked = 0usize;
        let mut honest_ok = true;
        let mut tamper_rejected = true;
        let mut survivors_arith = 0usize;
        let mut survivors_total = 0usize;
        let mut swept = false;
        for record in &records {
            let events = record.get_precompile_events(key);
            if events.is_empty() {
                continue;
            }
            // Executor results against the reference, in call order.
            for (_, event) in events.iter() {
                let e = match event {
                    PrecompileEvent::PallasFp(e) | PrecompileEvent::VestaFp(e) => e,
                    _ => bail!("{name}: unexpected event"),
                };
                let result = point_from_words(
                    &e.x_memory_records.iter().map(|r| r.value).chain([0; 4]).collect::<Vec<_>>(),
                )
                .0;
                let x = point_from_words(&e.x.iter().copied().chain([0; 4]).collect::<Vec<_>>()).0;
                let y = point_from_words(&e.y.iter().copied().chain([0; 4]).collect::<Vec<_>>()).0;
                let expected = match e.op {
                    sp1_core_executor::events::FieldOperation::Add => (&x + &y) % &modulus,
                    sp1_core_executor::events::FieldOperation::Sub => (&x + &modulus - &y) % &modulus,
                    sp1_core_executor::events::FieldOperation::Mul => (&x * &y) % &modulus,
                    _ => bail!("{name}: unexpected operation"),
                };
                ensure!(result == expected, "{name}: executor result differs from the reference");
                checked += 1;
            }
            let trace = match key {
                SyscallCode::PALLAS_FP_ADD => MachineAir::<SP1Field>::generate_trace(
                    &FpOpChip::<PallasBaseField, SupervisorMode>::new(),
                    record,
                    &mut ExecutionRecord::default(),
                ),
                _ => MachineAir::<SP1Field>::generate_trace(
                    &FpOpChip::<VestaBaseField, SupervisorMode>::new(),
                    record,
                    &mut ExecutionRecord::default(),
                ),
            };
            honest_ok &= eval_rows(chip, trace.clone()).is_empty();
            // Tamper the first row's result: the last arithmetic block starts
            // with the output's result limbs.
            let result_column = width - arithmetic;
            let mut tampered = trace.clone();
            tampered.values[result_column] += SP1Field::one();
            tamper_rejected &= !eval_rows(chip, tampered).is_empty();
            if !swept {
                for column in 0..width {
                    let mut t = trace.clone();
                    t.values[column] += SP1Field::from_canonical_u32(1 + (rng.next_u64() % 1000) as u32);
                    if eval_rows(chip, t).is_empty() {
                        survivors_total += 1;
                        if column >= width - arithmetic {
                            survivors_arith += 1;
                        }
                    }
                }
                swept = true;
            }
        }
        ensure!(honest_ok, "{name}: an honest row fails the AIR");
        ensure!(tamper_rejected, "{name}: a tampered result limb satisfies the AIR");
        ensure!(survivors_arith == 0, "{name}: {survivors_arith} arithmetic columns survive perturbation");
        fields.push(json!({
            "chip": name, "width": width, "events_checked": checked,
            "honest_rows_satisfy_air": honest_ok, "tampered_result_rejected": tamper_rejected,
            "surviving_columns": survivors_total, "surviving_arithmetic_columns": survivors_arith,
        }));
    }
    ensure!(
        fields.iter().map(|f| f["events_checked"].as_u64().unwrap()).sum::<u64>() as usize == calls.len(),
        "not every call produced a checked event"
    );

    // CPU core proof of the same program, checked by the complete verifier
    // against the digest the program commits.
    let started = Instant::now();
    let (proof, vk, prover, shard_records, proof_cycles) = prove_program(program, core_opts()).await?;
    let policy = Policy {
        require_successful_exit: true,
        expected_committed_digest: Some(fixture_digest_bytes(tag)),
    };
    let verdict = verify_complete_execution(&prover, &vk, &proof, &policy);
    ensure!(verdict.is_ok(), "complete verifier rejected the honest Pasta Fp proof");
    let report = json!({
        "mode": "pasta-fp-check",
        "calls": calls.len(),
        "guest_cycles": cycles,
        "chips": fields,
        "proof": {
            "shards": shard_records, "cycles": proof_cycles,
            "complete_verifier_accepted": true,
            "proof_seconds": started.elapsed().as_secs_f64(),
            "kind": "CPU core proof of a synthetic program",
        },
    });
    fs::create_dir_all(output)?;
    write_json(output.join("result.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Mode: pipeline-cpu
//
// Local CPU check of the proving pipeline: one multi-shard Pasta program is
// proved twice, first with the original single-threaded producer and one shard
// proof at a time, then with the pipelined producer and several shard proofs
// in flight. Both proofs must pass the complete-execution verifier, and the
// two producers must emit byte-identical record streams. CPU proofs of a small
// synthetic program only; no GPU, no mainnet guest proof.
// ---------------------------------------------------------------------------

async fn run_pipeline_cpu(output: &Path) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    let calls_wanted = fuzz_env_u64("PASTA_PIPELINE_CALLS", 256) as usize;
    let depth = fuzz_env_u64("PASTA_PROVE_INFLIGHT", 3) as usize;
    let add_op = OPS[0];
    let pts = multiples(add_op.curve, 4);
    let calls: Vec<Call> = (0..calls_wanted)
        .map(|i| make_call(add_op, pts[i % 3].clone(), Some(pts[i % 3 + 1].clone())))
        .collect::<Result<_>>()?;
    let (program, _) = build_program_at(&calls, FIXTURE_BASE, true, false, 0, Some(0), 0, 0)?;

    let verifier = ShardVerifier::from_basefold_parameters(
        FriConfig::default_fri_config(),
        LOG_STACKING_HEIGHT,
        MAX_LOG_ROW_COUNT,
        Air::machine(),
    );
    let prover = SimpleProver::new(verifier.clone(), Cpu::new(verifier));
    let (pk, vk) = prover.setup(program.clone()).await;
    let pk = unsafe { pk.into_inner() };
    let policy = Policy { require_successful_exit: true, expected_committed_digest: None };

    // Control: the sequential producer's canonical digest must be stable run
    // to run, or a mismatch below would not be attributable to the pipeline.
    std::env::set_var("PASTA_REPLAY_THREADS", "1");
    let control_a = record_stream_digest(&program, multishard_opts())?;
    let control_b = record_stream_digest(&program, multishard_opts())?;
    std::env::remove_var("PASTA_REPLAY_THREADS");
    ensure!(control_a == control_b, "the sequential producer's canonical digest is not stable");

    let mut arms = Vec::new();
    let mut digests = Vec::new();
    for (name, replay, prove_depth) in [("sequential", "1", 1usize), ("pipelined", "0", depth)] {
        // `0` is not a valid thread count, so the producer falls back to its
        // default parallelism; `1` selects the original inline loop.
        std::env::set_var("PASTA_REPLAY_THREADS", replay);
        if replay == "0" {
            std::env::remove_var("PASTA_REPLAY_THREADS");
        }
        let started = Instant::now();
        let (tx, rx) = tokio::sync::mpsc::channel::<ExecutionRecord>(4);
        let producer_program = program.clone();
        let producer = tokio::task::spawn_blocking(move || {
            sp1_core_machine::utils::generate_records_streaming_with_summary::<SP1Field, _>(
                producer_program,
                SP1Stdin::new(),
                multishard_opts(),
                [0; 4],
                move |record| tx.blocking_send(record).is_ok(),
            )
        });
        let (shards, consumer) = pipeline::prove_stream(&prover, pk.clone(), rx, prove_depth).await;
        let summary = producer
            .await
            .map_err(|e| anyhow::anyhow!("producer panicked: {e}"))?
            .map_err(|e| anyhow::anyhow!("execution failed: {e}"))?;
        let generated = shards.len();
        let proof = MachineProof { shard_proofs: canonical_shard_order(shards) };
        ensure!(proof.shard_proofs.len() == generated, "ordering lost shards");
        let verdict = verify_complete_execution(&prover, &vk, &proof, &policy);
        let seconds = started.elapsed().as_secs_f64();
        let proof_bytes = bincode::serialize(&proof)?;
        let record_stream = record_stream_digest(&program, multishard_opts())?;
        digests.push(record_stream.clone());
        arms.push(json!({
            "arm": name,
            "replay_threads_env": replay,
            "prove_depth": prove_depth,
            "guest_cycles": summary.cycles,
            "exit_code": summary.exit_code,
            "shards": generated,
            "complete_verifier_accepted": verdict.is_ok(),
            "rejection_check": verdict.as_ref().err().map(|e| e.check),
            "proof_sha256": sha(&proof_bytes),
            "record_stream_sha256": record_stream,
            "producer_profile": pipeline::producer_profile_json(&summary.profile),
            "consumer": consumer.to_json(),
            "seconds": seconds,
        }));
        ensure!(verdict.is_ok(), "{name}: complete verifier rejected the proof");
    }
    std::env::remove_var("PASTA_REPLAY_THREADS");
    ensure!(digests[0] == digests[1], "the two producers emitted different record streams");
    fs::create_dir_all(output)?;
    let report = json!({
        "mode": "pipeline-cpu",
        "calls": calls_wanted,
        "proof_generated": "CPU core proofs of a synthetic program",
        "gpu_used": false,
        "record_streams_identical": true,
        "sequential_digest_stable": true,
        "arms": arms,
    });
    write_json(output.join("result.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// Fold one record into `hasher` canonically. A record's bincode is not
/// canonical: `byte_lookups` and the precompile-event map are hash maps whose
/// iteration order differs between instances, and precompile shards' global
/// interaction events inherit that order. Everything else is hashed as
/// bincode with those two fields taken out, then the lookups as sorted
/// serialized entries and the precompile events by syscall code, each event
/// list in its own order. The record is left exactly as it was.
fn hash_record_canonical(hasher: &mut Sha256, record: &mut ExecutionRecord) {
    let lookups = std::mem::take(&mut record.byte_lookups);
    let precompiles = std::mem::take(&mut record.precompile_events);
    // Upstream builds precompile shards' global interaction events by iterating
    // the precompile-event hash map, whose hasher is seeded per process, so
    // their order differs between processes for the same execution. The global
    // sum is order-independent; hash them sorted.
    let globals = std::mem::take(&mut record.global_interaction_events);
    bincode::serialize_into(&mut *hasher, &*record).expect("record serializes");
    let mut sorted: Vec<Vec<u8>> = globals
        .iter()
        .map(|event| bincode::serialize(event).expect("global event serializes"))
        .collect();
    sorted.sort_unstable();
    hasher.update((sorted.len() as u64).to_le_bytes());
    for event in &sorted {
        hasher.update(event);
    }
    record.global_interaction_events = globals;
    let mut entries: Vec<Vec<u8>> = lookups
        .iter()
        .map(|entry| bincode::serialize(&entry).expect("lookup serializes"))
        .collect();
    entries.sort_unstable();
    hasher.update((entries.len() as u64).to_le_bytes());
    for entry in &entries {
        hasher.update(entry);
    }
    let mut codes: Vec<_> = precompiles.events.iter().collect();
    codes.sort_by_key(|(code, _)| **code as u32);
    hasher.update((codes.len() as u64).to_le_bytes());
    for (code, events) in codes {
        hasher.update((*code as u32).to_le_bytes());
        bincode::serialize_into(&mut *hasher, events).expect("events serialize");
    }
    record.byte_lookups = lookups;
    record.precompile_events = precompiles;
}

/// Per-record digests of separate field groups, so a producer mismatch can be
/// localized to the record and the part of it that differs.
fn record_field_digests(record: &ExecutionRecord) -> Value {
    let h = |bytes: Vec<u8>| hex::encode(Sha256::digest(bytes))[..16].to_string();
    let mut lookups: Vec<Vec<u8>> = record
        .byte_lookups
        .iter()
        .map(|entry| bincode::serialize(&entry).expect("lookup"))
        .collect();
    lookups.sort_unstable();
    let mut codes: Vec<_> = record.precompile_events.events.iter().collect();
    codes.sort_by_key(|(code, _)| **code as u32);
    let precompiles: Vec<u8> = codes
        .iter()
        .flat_map(|(code, events)| {
            let mut v = (**code as u32).to_le_bytes().to_vec();
            v.extend(bincode::serialize(events).expect("events"));
            v
        })
        .collect();
    let mut global: Vec<Vec<u8>> = record
        .global_interaction_events
        .iter()
        .map(|e| bincode::serialize(e).expect("global"))
        .collect();
    let global_ordered = h(global.concat());
    global.sort_unstable();
    json!({
        "public_values": h(bincode::serialize(&record.public_values).expect("pv")),
        "byte_lookups": h(lookups.concat()),
        "precompiles": h(precompiles),
        "global_interactions_ordered": global_ordered,
        "global_interactions_sorted": h(global.concat()),
        "global_memory_init": h(bincode::serialize(&record.global_memory_initialize_events).expect("init")),
        "global_memory_final": h(bincode::serialize(&record.global_memory_finalize_events).expect("final")),
        "cpu_local_memory": h(bincode::serialize(&record.cpu_local_memory_access).expect("local")),
        "cumulative_sum": h(bincode::serialize(&*record.global_cumulative_sum.lock().unwrap()).expect("sum")),
        "counts": [record.cpu_event_count, record.global_interaction_event_count],
    })
}

fn run_producer_equivalence(output: &Path, elf: Option<&Path>, input: Option<&Path>) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    // Either a real guest (`--elf`, `--input`, options from the environment as
    // in r5-cycles) or a synthetic Pasta program with small shards.
    let (program, stdin, opts, calls) = if let Some(elf) = elf {
        let elf = fs::read(elf)?;
        let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);
        let dir = input.context("--input is required with --elf")?;
        let mut stdin = SP1Stdin::new();
        stdin.write_slice(&fs::read(dir.join("proof-input.bin"))?);
        stdin.write_slice(&fs::read(dir.join("receipt-input.bin"))?);
        let mut opts = SP1CoreOpts::default();
        opts.global_dependencies_opt =
            std::env::var("PASTA_GLOBAL_DEPS_OPT").map(|v| v != "0").unwrap_or(false);
        (program, stdin, opts, 0usize)
    } else {
        let mut calls = Vec::new();
        for op in &OPS {
            let pts = multiples(op.curve, 80);
            for i in 0..60 {
                calls.push(match op.kind {
                    Kind::Add => make_call(*op, pts[i].clone(), Some(pts[i + 7].clone()))?,
                    Kind::Double => make_call(*op, pts[i].clone(), None)?,
                });
            }
        }
        let (program, _) = build_program(&calls)?;
        (program, SP1Stdin::new(), core_opts(), calls.len())
    };
    let collect = |threads: &str| -> Result<Vec<Value>> {
        std::env::set_var("PASTA_REPLAY_THREADS", threads);
        let mut out = Vec::new();
        sp1_core_machine::utils::generate_records_streaming::<SP1Field, _>(
            program.clone(),
            stdin.clone(),
            opts.clone(),
            [0; 4],
            |mut record| {
                let mut fields = record_field_digests(&record);
                let mut hasher = Sha256::new();
                hash_record_canonical(&mut hasher, &mut record);
                fields["canonical"] = json!(hex::encode(hasher.finalize())[..16].to_string());
                out.push(fields);
                true
            },
        )
        .map_err(|e| anyhow::anyhow!("execution failed: {e}"))?;
        std::env::remove_var("PASTA_REPLAY_THREADS");
        Ok(out)
    };
    let sequential = collect("1")?;
    let sequential_again = collect("1")?;
    let pipelined = collect("8")?;
    let diff = |a: &[Value], b: &[Value]| -> Vec<Value> {
        let mut out = Vec::new();
        if a.len() != b.len() {
            out.push(json!({"record_count": [a.len(), b.len()]}));
        }
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            let fields: Vec<&String> = x
                .as_object()
                .unwrap()
                .keys()
                .filter(|k| x[*k] != y[*k])
                .collect();
            if !fields.is_empty() {
                out.push(json!({"record": i, "fields": fields}));
            }
        }
        out
    };
    let report = json!({
        "mode": "producer-equivalence",
        "calls": calls,
        "records": sequential.len(),
        "sequential_vs_sequential": diff(&sequential, &sequential_again),
        "sequential_vs_pipelined": diff(&sequential, &pipelined),
    });
    fs::create_dir_all(output)?;
    write_json(output.join("result.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// SHA-256 over the bincode of every record the current producer settings
/// emit for `program`, in order.
fn record_stream_digest(program: &Arc<Program>, opts: SP1CoreOpts) -> Result<String> {
    let mut hasher = Sha256::new();
    sp1_core_machine::utils::generate_records_streaming::<SP1Field, _>(
        program.clone(),
        SP1Stdin::new(),
        opts,
        [0; 4],
        |mut record| {
            hash_record_canonical(&mut hasher, &mut record);
            true
        },
    )
    .map_err(|e| anyhow::anyhow!("execution failed: {e}"))?;
    Ok(hex::encode(hasher.finalize()))
}

// ---------------------------------------------------------------------------
// Mode: pasta-fuzz
//
// Differential fuzzing of the Pasta precompiles against the independent
// integer reference (`ref_add` / `ref_double`), in three parts:
//
// 1. executor: the curve operation the host executor runs (`ec_add`,
//    `ec_double`) against the generic short-Weierstrass formulas and the
//    reference, including degenerate inputs, which must behave identically;
// 2. witness: random operands executed by a real program; every result the
//    executor writes and every trace row's x3/y3 limbs must equal the
//    reference, and every honest row must satisfy the chip AIR;
// 3. columns: every column of an honest row is perturbed in turn and the
//    chip AIR's algebraic constraints are evaluated. A column that survives
//    is constrained, if at all, only by the interaction arguments (memory,
//    syscall, byte lookups), which this sweep does not evaluate.
//
// Inputs come from a dependency-free splitmix64 stream; the seed is printed
// and recorded so any case replays exactly. No proof is generated.
// ---------------------------------------------------------------------------

struct FuzzRng(u64);

impl FuzzRng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `kG` for a random 256-bit `k`, computed with the reference group law.
    fn point(&mut self, curve: Curve) -> Point {
        let m = modulus(curve);
        let g = generator(curve);
        loop {
            let mut acc = GroupPoint::Infinity;
            let mut base = GroupPoint::Finite(g.0.clone(), g.1.clone());
            for _ in 0..4 {
                let word = self.next_u64();
                for bit in 0..64 {
                    if (word >> bit) & 1 == 1 {
                        acc = group_add(&acc, &base, &m);
                    }
                    base = group_add(&base, &base, &m);
                }
            }
            if let GroupPoint::Finite(x, y) = acc {
                return (x, y);
            }
        }
    }

    /// A random operand pair with distinct x coordinates, as the raw add requires.
    fn add_operands(&mut self, curve: Curve) -> (Point, Point) {
        loop {
            let (p, q) = (self.point(curve), self.point(curve));
            if p.0 != q.0 {
                return (p, q);
            }
        }
    }
}

fn fuzz_env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn run_pasta_fuzz(output: &Path) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    let seed = fuzz_env_u64("PASTA_FUZZ_SEED", 0x5eed_0001);
    let iterations = fuzz_env_u64("PASTA_FUZZ_ITERS", 32) as usize;
    eprintln!("pasta-fuzz seed={seed:#x} iterations={iterations}");
    let started = Instant::now();
    let mut rng = FuzzRng(seed);
    let executor = fuzz_executor(&mut rng, iterations)?;
    let witness = fuzz_witness(&mut rng, iterations)?;
    let columns = fuzz_columns(&mut rng)?;
    fs::create_dir_all(output)?;
    let report = json!({
        "mode": "pasta-fuzz",
        "seed": format!("{seed:#x}"),
        "iterations_per_operation": iterations,
        "proved": false,
        "executor": executor,
        "witness": witness,
        "columns": columns,
        "seconds": started.elapsed().as_secs_f64(),
    });
    write_json(output.join("result.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn affine<E: sp1_curves::EllipticCurve>(p: &Point) -> sp1_curves::AffinePoint<E> {
    sp1_curves::AffinePoint::new(p.0.clone(), p.1.clone())
}

fn point_of<E: sp1_curves::EllipticCurve>(p: &sp1_curves::AffinePoint<E>) -> Point {
    (p.x.clone(), p.y.clone())
}

/// Whether two computations agree, treating "both panicked" as agreement.
fn same_outcome(a: std::thread::Result<Point>, b: std::thread::Result<Point>) -> bool {
    match (a, b) {
        (Ok(a), Ok(b)) => a == b,
        (Err(_), Err(_)) => true,
        _ => false,
    }
}

/// Executor curve operations against the generic formulas and the reference.
fn fuzz_executor(rng: &mut FuzzRng, iterations: usize) -> Result<Value> {
    use sp1_curves::EllipticCurve;
    fn run<E: sp1_curves::weierstrass::WeierstrassParameters>(
        rng: &mut FuzzRng,
        curve: Curve,
        iterations: usize,
    ) -> Result<Value>
    where
        sp1_curves::weierstrass::SwCurve<E>: EllipticCurve,
    {
        type Sw<E> = sp1_curves::weierstrass::SwCurve<E>;
        let m = modulus(curve);
        for i in 0..iterations {
            let (p, q) = rng.add_operands(curve);
            let expected = ref_add(&p, &q, &m);
            let fast = point_of(&Sw::<E>::ec_add(&affine(&p), &affine(&q)));
            let generic = point_of(&affine::<Sw<E>>(&p).sw_add(&affine(&q)));
            ensure!(fast == expected, "{curve:?} add {i}: executor differs from reference");
            ensure!(generic == expected, "{curve:?} add {i}: generic differs from reference");
            let expected = ref_double(&p, &m);
            let fast = point_of(&Sw::<E>::ec_double(&affine(&p)));
            let generic = point_of(&affine::<Sw<E>>(&p).sw_double());
            ensure!(fast == expected, "{curve:?} double {i}: executor differs from reference");
            ensure!(generic == expected, "{curve:?} double {i}: generic differs from reference");
        }
        // Degenerate inputs the AIR rejects but the executor still evaluates:
        // the executor must behave exactly as the generic path does, panics
        // included, so moving to the fast path cannot change what a malformed
        // guest observes.
        let p = rng.point(curve);
        let minus_p = (p.0.clone(), &m - &p.1);
        let off_curve = (p.0.clone(), BigUint::zero());
        let noncanonical = (&p.0 + &m, p.1.clone());
        let cases: Vec<(&str, Point, Option<Point>)> = vec![
            ("add-p-minus-p", p.clone(), Some(minus_p)),
            ("add-equal-points", p.clone(), Some(p.clone())),
            ("add-noncanonical-q", p.clone(), Some(noncanonical.clone())),
            ("add-noncanonical-p", noncanonical.clone(), Some(p.clone())),
            ("double-zero-y", off_curve, None),
            ("double-noncanonical", noncanonical, None),
        ];
        let mut degenerate = Vec::new();
        for (name, a, b) in cases {
            let (fast, generic) = match &b {
                Some(b) => (
                    catch_unwind(AssertUnwindSafe(|| point_of(&Sw::<E>::ec_add(&affine(&a), &affine(b))))),
                    catch_unwind(AssertUnwindSafe(|| point_of(&affine::<Sw<E>>(&a).sw_add(&affine(b))))),
                ),
                None => (
                    catch_unwind(AssertUnwindSafe(|| point_of(&Sw::<E>::ec_double(&affine(&a))))),
                    catch_unwind(AssertUnwindSafe(|| point_of(&affine::<Sw<E>>(&a).sw_double()))),
                ),
            };
            let panicked = (fast.is_err(), generic.is_err());
            ensure!(same_outcome(fast, generic), "{curve:?} {name}: executor and generic paths diverge");
            degenerate.push(json!({"case": name, "executor_panicked": panicked.0, "generic_panicked": panicked.1}));
        }
        Ok(json!({"random_cases": iterations, "degenerate": degenerate}))
    }
    // Panics are expected in the degenerate cases; keep their messages out of the log.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = (|| {
        Ok(json!({
            "vesta": run::<sp1_curves::weierstrass::pasta::VestaParameters>(rng, Curve::Vesta, iterations)?,
            "pallas": run::<sp1_curves::weierstrass::pasta::PallasParameters>(rng, Curve::Pallas, iterations)?,
        }))
    })();
    std::panic::set_hook(hook);
    result
}

/// The x3 and y3 results a trace row carries, read from its byte limbs.
fn row_result(op: &Op, row: &[SP1Field]) -> Point {
    fn value<F: PrimeField32>(limbs: &[F]) -> BigUint {
        let bytes: Vec<u8> = limbs.iter().map(|l| l.as_canonical_u32() as u8).collect();
        BigUint::from_bytes_le(&bytes)
    }
    macro_rules! read {
        ($ty:ty) => {{
            let cols: &$ty = row.borrow();
            (value(&cols.x3_ins.result.0), value(&cols.y3_ins.result.0))
        }};
    }
    use sp1_core_machine::SupervisorMode as S;
    match (op.curve, op.kind) {
        (Curve::Vesta, Kind::Add) => read!(WeierstrassAddAssignCols<SP1Field, VestaBaseField, S>),
        (Curve::Pallas, Kind::Add) => read!(WeierstrassAddAssignCols<SP1Field, PallasBaseField, S>),
        (Curve::Vesta, Kind::Double) => read!(WeierstrassDoubleAssignCols<SP1Field, VestaBaseField, S>),
        (Curve::Pallas, Kind::Double) => read!(WeierstrassDoubleAssignCols<SP1Field, PallasBaseField, S>),
    }
}

fn chip_for<'a>(
    machine: &'a sp1_hypercube::Machine<SP1Field, Air>,
    op: &Op,
) -> Result<&'a Chip<SP1Field, Air>> {
    machine
        .chips()
        .iter()
        .find(|c| c.name() == op.chip)
        .with_context(|| format!("{} chip not in machine", op.chip))
}

/// Random operands through a real program: executor results, witness rows and
/// honest-row constraints, all against the reference.
fn fuzz_witness(rng: &mut FuzzRng, iterations: usize) -> Result<Value> {
    let mut results = Vec::new();
    for op in &OPS {
        let calls: Vec<Call> = (0..iterations)
            .map(|_| match op.kind {
                Kind::Add => {
                    let (p, q) = rng.add_operands(op.curve);
                    make_call(*op, p, Some(q))
                }
                Kind::Double => make_call(*op, rng.point(op.curve), None),
            })
            .collect::<Result<_>>()?;
        let (program, _) = build_program(&calls)?;
        let (records, _) =
            generate_records::<SP1Field>(program, SP1Stdin::new(), core_opts(), [0; 4])?;
        let machine = Air::machine();
        let chip = chip_for(&machine, op)?;
        let width = op_width(op);
        let mut checked = 0usize;
        for record in &records {
            let events = record.get_precompile_events(op.syscall);
            if events.is_empty() {
                continue;
            }
            let trace = generate_chip_trace(&mut record.clone(), op);
            let failures = eval_rows(chip, trace.clone());
            ensure!(failures.is_empty(), "{}: honest rows fail the AIR: {} rows", op.id, failures.len());
            for (row, (_, event)) in events.iter().enumerate() {
                let written = match event {
                    PrecompileEvent::VestaAdd(e) | PrecompileEvent::PallasAdd(e) => {
                        e.p_memory_records.iter().map(|r| r.value).collect::<Vec<_>>()
                    }
                    PrecompileEvent::VestaDouble(e) | PrecompileEvent::PallasDouble(e) => {
                        e.p_memory_records.iter().map(|r| r.value).collect::<Vec<_>>()
                    }
                    _ => bail!("{}: unexpected event kind", op.id),
                };
                let expected = &calls[checked].expected;
                ensure!(
                    point_from_words(&written) == *expected,
                    "{} call {checked}: executor result differs from reference",
                    op.id
                );
                ensure!(
                    row_result(op, &trace.values[row * width..(row + 1) * width]) == *expected,
                    "{} call {checked}: witness x3/y3 differ from reference",
                    op.id
                );
                checked += 1;
            }
        }
        ensure!(checked == iterations, "{}: checked {checked} of {iterations} calls", op.id);
        results.push(json!({"operation": op.id, "calls": checked, "executor_matches": true,
            "witness_matches": true, "honest_rows_satisfy_air": true}));
    }
    Ok(json!(results))
}

/// Column offsets of a chip's row struct, from `offset_of!` on its `u8`
/// instantiation (the columns are `repr(C)` arrays of `T`).
fn column_fields(op: &Op) -> Vec<(&'static str, usize)> {
    use sp1_core_machine::SupervisorMode as S;
    macro_rules! fields {
        ($ty:ty; $($f:ident),*) => {{
            let mut v = vec![$((stringify!($f), core::mem::offset_of!($ty, $f))),*];
            v.sort_by_key(|(_, o)| *o);
            v
        }};
    }
    macro_rules! add_fields {
        ($b:ty) => { fields!(WeierstrassAddAssignCols<u8, $b, S>; is_real, clk_high, clk_low, p_ptr, q_ptr,
            p_addrs, q_addrs, p_access, q_access, slope_denominator, inverse_check, slope_numerator, slope,
            slope_squared, p_x_plus_q_x, x3_ins, p_x_minus_x, y3_ins, slope_times_p_x_minus_x, x3_range,
            y3_range, read_slice_page_prot_access, write_slice_page_prot_access) };
    }
    macro_rules! double_fields {
        ($b:ty) => { fields!(WeierstrassDoubleAssignCols<u8, $b, S>; is_real, clk_high, clk_low, p_ptr,
            p_addrs, p_access, slope_denominator, slope_numerator, slope, p_x_squared, p_x_squared_times_3,
            slope_squared, p_x_plus_p_x, x3_ins, p_x_minus_x, y3_ins, slope_times_p_x_minus_x, x3_range,
            y3_range, write_slice_page_prot_access) };
    }
    match (op.curve, op.kind) {
        (Curve::Vesta, Kind::Add) => add_fields!(VestaBaseField),
        (Curve::Pallas, Kind::Add) => add_fields!(PallasBaseField),
        (Curve::Vesta, Kind::Double) => double_fields!(VestaBaseField),
        (Curve::Pallas, Kind::Double) => double_fields!(PallasBaseField),
    }
}

fn column_field(fields: &[(&'static str, usize)], column: usize) -> &'static str {
    fields.iter().rev().find(|(_, o)| *o <= column).map(|(n, _)| *n).unwrap_or("?")
}

/// Fields whose values are bound by the memory, syscall or page-protection
/// interactions rather than by the chip's own algebra.
const INTERACTION_BOUND_FIELDS: [&str; 10] = [
    "clk_high", "clk_low", "p_ptr", "q_ptr", "p_addrs", "q_addrs", "p_access", "q_access",
    "read_slice_page_prot_access", "write_slice_page_prot_access",
];

/// Perturb every column of one honest row, twice, and evaluate the AIR.
fn fuzz_columns(rng: &mut FuzzRng) -> Result<Value> {
    let mut results = Vec::new();
    for op in &OPS {
        let call = match op.kind {
            Kind::Add => {
                let (p, q) = rng.add_operands(op.curve);
                make_call(*op, p, Some(q))?
            }
            Kind::Double => make_call(*op, rng.point(op.curve), None)?,
        };
        let (program, _) = build_program(std::slice::from_ref(&call))?;
        let (records, _) =
            generate_records::<SP1Field>(program, SP1Stdin::new(), core_opts(), [0; 4])?;
        let record = records
            .iter()
            .find(|r| !r.get_precompile_events(op.syscall).is_empty())
            .context("no shard carried the precompile event")?;
        let machine = Air::machine();
        let chip = chip_for(&machine, op)?;
        let width = op_width(op);
        let honest = generate_chip_trace(&mut record.clone(), op);
        ensure!(eval_rows(chip, honest.clone()).is_empty(), "{}: honest row fails", op.id);
        let fields = column_fields(op);
        let mut surviving: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for column in 0..width {
            // A unit step and an arbitrary nonzero step: a column counts as
            // algebraically unconstrained only if both leave the AIR satisfied.
            let random = 1 + (rng.next_u64() % (SP1Field::ORDER_U32 as u64 - 1)) as u32;
            let survives = [1u32, random].iter().all(|delta| {
                let mut trace = honest.clone();
                trace.values[column] += SP1Field::from_canonical_u32(*delta);
                eval_rows(chip, trace).is_empty()
            });
            if survives {
                surviving.entry(column_field(&fields, column)).or_default().push(column);
            }
        }
        let arithmetic: Vec<&str> = surviving
            .keys()
            .copied()
            .filter(|f| !INTERACTION_BOUND_FIELDS.contains(f))
            .collect();
        // The recorded expectation is that no arithmetic column survives
        // (docs/pasta-air-verification-plan.md); a new survivor is a finding.
        ensure!(
            arithmetic.is_empty(),
            "{}: algebraically unconstrained arithmetic fields {:?}",
            op.id,
            arithmetic
        );
        results.push(json!({
            "operation": op.id,
            "columns": width,
            "surviving_columns": surviving.values().map(Vec::len).sum::<usize>(),
            "surviving_by_field": surviving,
            "algebraically_unconstrained_arithmetic_fields": arithmetic,
        }));
    }
    Ok(json!({
        "scope": "algebraic row constraints only; interaction-bound fields are expected to survive",
        "interaction_bound_fields": INTERACTION_BOUND_FIELDS,
        "results": results,
    }))
}

fn registration_report() -> Result<Value> {
    let costs = rv64im_costs();
    let machine = Air::machine();
    let names: Vec<String> = machine
        .chips()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let mut missing = Vec::new();
    for id in RiscvAirId::iter() {
        if costs.get(&id).is_none() {
            missing.push(id.to_string());
        }
    }
    ensure!(
        missing.is_empty(),
        "AIR IDs missing cost entries: {missing:?}"
    );
    let mut widths = Vec::new();
    for op in &OPS {
        let actual = op_width(op);
        let registered = *costs.get(&op.air).context("cost entry")?;
        ensure!(
            registered == actual,
            "{} registered cost {registered} != actual AIR width {actual}",
            op.chip
        );
        ensure!(
            names.iter().any(|n| n == op.chip),
            "{} chip is not registered",
            op.chip
        );
        ensure!(
            op.syscall.as_air_id() == Some(op.air),
            "{} syscall does not map to its AIR id",
            op.id
        );
        widths.push(json!({
            "operation": op.id,
            "chip": op.chip,
            "air_id": op.air.to_string(),
            "syscall_code": format!("0x{:08x}", op.syscall as u32),
            "syscall_id": op.syscall.syscall_id(),
            "registered_trace_cost": registered,
            "actual_air_width": actual,
            "touched_addresses": op.syscall.touched_addresses(),
            "touched_pages": op.syscall.touched_pages(),
        }));
    }
    // Syscall ids must not collide with any other syscall.
    let mut seen = BTreeMap::new();
    for code in SyscallCode::iter() {
        if let Some(previous) = seen.insert(code.syscall_id(), code) {
            bail!("syscall id collision: {previous:?} and {code:?}");
        }
    }
    // Every chip's registered trace cost against its actual AIR width. Chips
    // added by this fork (Pasta curve ops and the fixed-modulus Pasta field
    // ops) must match exactly; upstream entries are reported, not enforced.
    let mut cost_mismatches = Vec::new();
    for chip in machine.chips() {
        let name = chip.name().to_string();
        let Some(id) = RiscvAirId::iter().find(|id| id.to_string() == name) else {
            continue;
        };
        let (cost, width) = (costs.get(&id).copied(), slop_air::BaseAir::<SP1Field>::width(chip));
        if cost != Some(width) {
            cost_mismatches.push(json!({"chip": name, "registered": cost, "width": width}));
        }
    }
    for fork_chip in ["PallasFpOpAssign", "PallasFpOpAssignUser", "VestaFpOpAssign", "VestaFpOpAssignUser"] {
        ensure!(
            !cost_mismatches.iter().any(|m| m["chip"] == fork_chip),
            "{fork_chip}: registered cost differs from its AIR width"
        );
        ensure!(names.iter().any(|n| n == fork_chip), "{fork_chip} is not registered");
    }
    Ok(json!({
        "chip_cost_mismatches": cost_mismatches,
        "air_ids_total": RiscvAirId::iter().count(),
        "air_ids_with_cost_entries": RiscvAirId::iter().count(),
        "registered_chips": names.len(),
        "syscall_ids_unique": true,
        "operations": widths,
    }))
}

// ---------------------------------------------------------------------------
// Mode: prove
// ---------------------------------------------------------------------------

async fn run_prove(output: &Path, selected: &[Op]) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    fs::create_dir_all(output)?;
    let suite_started = Instant::now();
    let registration = registration_report()?;
    write_json(output.join("registration.json"), &registration)?;

    let mut operation_reports = Vec::new();
    for op in selected {
        println!("OP_START {}", op.id);
        let points = multiples(op.curve, 4);
        let call = match op.kind {
            // Distinct x coordinates: G and 2G.
            Kind::Add => make_call(*op, points[0].clone(), Some(points[1].clone()))?,
            Kind::Double => make_call(*op, points[0].clone(), None)?,
        };
        let tag = OPS.iter().position(|o| o.id == op.id).unwrap() as u32;
        let (program, fixture) = build_program_at(
            std::slice::from_ref(&call),
            FIXTURE_BASE,
            true,
            false,
            0,
            Some(tag),
            0,
            0,
        )?;
        let dir = output.join(op.id);
        fs::create_dir_all(&dir)?;
        write_json(dir.join("fixture.json"), &fixture)?;
        let prepared = prepare(program).await?;
        fs::write(dir.join("program.bin"), &prepared.program_bytes)?;
        fs::write(dir.join("verification-key.bin"), &prepared.vk_bytes)?;
        fs::write(
            dir.join("honest-records.bin"),
            bincode::serialize(&prepared.records)?,
        )?;

        let events: usize = prepared
            .records
            .iter()
            .map(|r| r.get_precompile_events(op.syscall).len())
            .sum();
        ensure!(
            events == 1,
            "{}: expected exactly one precompile event, got {events}",
            op.id
        );
        // The executor's own output must equal the independent reference.
        let expected = words(&call.expected);
        let mut checked = false;
        for record in &prepared.records {
            for (_, event) in record.get_precompile_events(op.syscall) {
                let records = match event {
                    PrecompileEvent::VestaAdd(e) | PrecompileEvent::PallasAdd(e) => {
                        &e.p_memory_records
                    }
                    PrecompileEvent::VestaDouble(e) | PrecompileEvent::PallasDouble(e) => {
                        &e.p_memory_records
                    }
                    _ => bail!("unexpected event variant for {}", op.id),
                };
                for (record, expected) in records.iter().zip(expected.iter()) {
                    ensure!(
                        record.value == *expected,
                        "{}: executor disagreed with the independent reference",
                        op.id
                    );
                }
                checked = true;
            }
        }
        ensure!(checked, "{}: no precompile event to check", op.id);

        let mut case_reports = Vec::new();
        for case in cases_for(op) {
            let started = Instant::now();
            let (proof, mutations) = prove_case(
                &prepared.cpu,
                prepared.pk.clone(),
                &prepared.records,
                op,
                case,
            )
            .await?;
            let prove_seconds = started.elapsed().as_secs_f64();
            let generated_shards = proof.shard_proofs.len();
            let proof_bytes = bincode::serialize(&proof)?;
            fs::write(dir.join(format!("{}.proof.bin", case.id())), &proof_bytes)?;
            let reloaded: Proof = bincode::deserialize(&proof_bytes)?;
            let serialized_shards = reloaded.shard_proofs.len();
            ensure!(
                serialized_shards == generated_shards && generated_shards == prepared.records.len(),
                "{} {}: shard assembly lost shards: {} records, {} generated, {} serialized",
                op.id,
                case.id(),
                prepared.records.len(),
                generated_shards,
                serialized_shards
            );
            let policy = Policy {
                require_successful_exit: true,
                expected_committed_digest: Some(fixture_digest_bytes(tag)),
            };
            let verify_started = Instant::now();
            let complete = verify_complete_execution(
                &prepared.prover,
                &prepared.restored_vk,
                &reloaded,
                &policy,
            );
            let verify_seconds = verify_started.elapsed().as_secs_f64();
            let accepted = complete.is_ok();
            // The weaker endpoint is recorded only to show what it would have
            // accepted; it is never the gate.
            let per_shard_accepted = prepared
                .prover
                .verify(&prepared.restored_vk, &reloaded)
                .is_ok();
            let rejection = complete.as_ref().err();
            let expected_acceptance = case == Case::Honest;
            let report = json!({
                "operation": op.id,
                "case": case.id(),
                "intended_coverage": case.covers(),
                "passed": accepted == expected_acceptance,
                "proof_generated": true,
                "proof_reloaded": true,
                "complete_verifier_accepted": accepted,
                "per_shard_verifier_accepted": per_shard_accepted,
                "expected_acceptance": expected_acceptance,
                "rejection_layer": rejection.map(|e| if e.check == "shard_cryptographic_verification" {
                    "shard_cryptographic_verification"
                } else {
                    "complete_execution_structure_or_policy"
                }),
                "rejection_check": rejection.map(|e| e.check),
                "expected_rejection_check": case.expected_rejection_check(),
                "verifier_error": rejection.map(|e| e.detail.clone()),
                "proof_bytes": proof_bytes.len(),
                "proof_sha256": sha(&proof_bytes),
                "execution_records": prepared.records.len(),
                "generated_shards": generated_shards,
                "serialized_shards": serialized_shards,
                "mutations": mutations,
                "prove_seconds": prove_seconds,
                "verify_seconds": verify_seconds,
            });
            write_json(dir.join(format!("{}.json", case.id())), &report)?;
            println!(
                "CASE {} {} accepted={accepted} prove_seconds={prove_seconds:.3}",
                op.id,
                case.id()
            );
            ensure!(
                accepted == expected_acceptance,
                "{} {}: unexpected complete-verifier result (accepted={accepted}, expected={expected_acceptance}, check={:?})",
                op.id,
                case.id(),
                rejection.map(|e| e.check)
            );
            if let Some(expected_check) = case.expected_rejection_check() {
                ensure!(
                    rejection.map(|e| e.check) == Some(expected_check),
                    "{} {}: expected rejection at {expected_check}, got {:?}",
                    op.id,
                    case.id(),
                    rejection.map(|e| e.check)
                );
            }
            case_reports.push(report);
        }

        let op_report = json!({
            "operation": op.id,
            "curve": format!("{:?}", op.curve),
            "kind": format!("{:?}", op.kind),
            "chip": op.chip,
            "air_id": op.air.to_string(),
            "syscall_code": format!("0x{:08x}", op.syscall as u32),
            "guest_cycles": prepared.cycles,
            "precompile_events": events,
            "shards": prepared.records.len(),
            "air_width": op_width(op),
            "program_sha256": sha(&prepared.program_bytes),
            "verification_key_sha256": sha(&prepared.vk_bytes),
            "trace_seconds": prepared.trace_seconds,
            "setup_seconds": prepared.setup_seconds,
            "cases": case_reports,
        });
        write_json(dir.join("result.json"), &op_report)?;
        operation_reports.push(op_report);
        println!("OP_PASSED {}", op.id);
    }

    let summary = json!({
        "status": "passed",
        "schema_version": 1,
        "mode": "prove",
        "scope": "real CPU SP1 core proofs of fixed-input synthetic Programs executing one Pasta precompile each",
        "not_in_scope": [
            "compiled ELF (separate artifact row)", "arbitrary-input curve validation in the raw syscall",
            "full Mina verification", "recursion", "compression", "Groth16", "GPU",
            "production soundness audit"
        ],
        "sp1_version": "6.1.0 with the isolated four-crate Pasta executor/machine fork",
        "log_stacking_height": LOG_STACKING_HEIGHT,
        "max_log_row_count": MAX_LOG_ROW_COUNT,
        "fri": {"log_blowup": 1, "num_queries": 94, "proof_of_work_bits": 16},
        "core_opts": core_opts(),
        "registration": registration,
        "operations": operation_reports,
        "total_seconds": suite_started.elapsed().as_secs_f64(),
    });
    write_json(output.join("result.json"), &summary)?;
    println!("SUITE_PASSED");
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Mode: verify (re-verify proofs saved on disk)
// ---------------------------------------------------------------------------

/// Reload downloaded core proofs without executing or proving the guest.
/// The ELF supplies the trusted key; saved keys are only equality checks.
async fn run_manifest_verify(
    manifest_path: &Path,
    elf_path: &Path,
    input: &Path,
    expected_public_values: &Path,
    output: &Path,
) -> Result<()> {
    use bincode::Options;
    use pasta_precompile_proof::run_manifest::{
        RunManifest, EXPECTED_LOG_STACKING_HEIGHT, EXPECTED_MAX_LOG_ROW_COUNT,
    };
    ensure!(!output.exists(), "use a fresh local-verification directory");
    fs::create_dir_all(output)?;
    let manifest_bytes = fs::read(manifest_path)?;
    let manifest = RunManifest::load(manifest_path)?;
    manifest.verify_elf(elf_path)?;
    let elf = fs::read(elf_path)?;
    let expected_bytes = fs::read(expected_public_values)?;
    ensure!(
        expected_bytes.len() == manifest.public_values_len,
        "expected public-value length"
    );
    ensure!(
        sha(&expected_bytes) == manifest.public_values_sha256,
        "expected public-value hash"
    );
    let expected_digest: [u8; 32] = Sha256::digest(&expected_bytes).into();
    let policy = Policy {
        require_successful_exit: true,
        expected_committed_digest: Some(expected_digest),
    };

    let verifier = ShardVerifier::from_basefold_parameters(
        sp1_primitives::fri_params::core_fri_config(),
        EXPECTED_LOG_STACKING_HEIGHT,
        EXPECTED_MAX_LOG_ROW_COUNT as usize,
        Air::machine(),
    );
    let prover = SimpleProver::new(verifier.clone(), Cpu::new(verifier));
    let setup_start = Instant::now();
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);
    let (pk, vk) = prover.setup(program).await;
    drop(pk);
    let setup_seconds = setup_start.elapsed().as_secs_f64();
    let derived_vk_bytes = bincode::serialize(&vk)?;
    let derived_vk_sha = sha(&derived_vk_bytes);
    fs::write(
        output.join("program-derived-verification-key.bin"),
        &derived_vk_bytes,
    )?;

    let empty: Proof = MachineProof {
        shard_proofs: Vec::new(),
    };
    let empty_error = verify_complete_execution(&prover, &vk, &empty, &policy)
        .expect_err("the complete verifier must reject an empty proof");
    ensure!(
        empty_error.check == "empty_proof",
        "wrong empty-proof rejecting check"
    );

    let mut results = Vec::new();
    for arm in &manifest.arms {
        let result = (|| -> Result<Value> {
            let dir = input.join(&arm.name);
            let proof_path = dir.join(format!("{}.proof.bin", arm.name));
            let receipt: Value = serde_json::from_slice(&fs::read(dir.join("result.json"))?)?;
            ensure!(
                receipt["arm"].as_str() == Some(arm.name.as_str()),
                "arm receipt mismatch"
            );
            ensure!(
                receipt["elf_sha256"].as_str() == Some(manifest.elf_sha256.as_str()),
                "receipt ELF mismatch"
            );
            let bytes = fs::read(&proof_path)?;
            let proof_sha = sha(&bytes);
            ensure!(
                receipt["proof_sha256"].as_str() == Some(proof_sha.as_str()),
                "downloaded proof hash mismatch"
            );
            ensure!(
                receipt["proof_bytes"].as_u64() == Some(bytes.len() as u64),
                "downloaded proof size mismatch"
            );
            let saved_vk = fs::read(dir.join("verification-key.bin"))?;
            ensure!(
                saved_vk == derived_vk_bytes,
                "saved key differs from the key derived from the pinned ELF"
            );
            let decode_start = Instant::now();
            let proof: Proof = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .reject_trailing_bytes()
                .deserialize(&bytes)?;
            let decode_seconds = decode_start.elapsed().as_secs_f64();
            let proof_size = bytes.len();
            drop(bytes);
            ensure!(
                proof.shard_proofs.len() == arm.expected_records,
                "reloaded shard count mismatch"
            );
            ensure!(
                receipt["generated_shards"].as_u64() == Some(proof.shard_proofs.len() as u64),
                "generated/reloaded shard count mismatch"
            );
            let mut chips = BTreeMap::<String, usize>::new();
            for shard in &proof.shard_proofs {
                for name in shard.opened_values.chips.keys() {
                    *chips.entry(name.clone()).or_default() += 1;
                }
            }
            for op in &OPS {
                ensure!(
                    chips.contains_key(op.chip),
                    "missing expected Pasta AIR {}",
                    op.chip
                );
            }
            ensure!(chips.contains_key("Uint256MulMod"), "missing UInt AIR");
            let verify_start = Instant::now();
            let verified = verify_complete_execution(&prover, &vk, &proof, &policy);
            let verify_seconds = verify_start.elapsed().as_secs_f64();
            let digest = committed_digest(&proof)?;
            let error = verified.as_ref().err();
            Ok(json!({
                "arm": arm.name, "status": if verified.is_ok() { "passed" } else { "failed" },
                "complete_verifier_accepted": verified.is_ok(),
                "rejection_check": error.map(|e| e.check), "verifier_error": error.map(|e| &e.detail),
                "proof_path": proof_path, "proof_sha256": proof_sha, "proof_bytes": proof_size,
                "reloaded_shards": proof.shard_proofs.len(), "expected_shards": arm.expected_records,
                "verification_key_sha256": derived_vk_sha,
                "verification_key_derived_from_pinned_elf": true, "saved_key_matches_derived": true,
                "committed_digest_hex": hex::encode(digest), "committed_digest_matches": digest == expected_digest,
                "chip_shard_counts": chips, "decode_seconds": decode_seconds,
                "cpu_verify_seconds": verify_seconds, "verified_on": "local-cpu",
            }))
        })();
        let row = result.unwrap_or_else(
            |error| json!({"arm": arm.name, "status": "failed", "error": format!("{error:#}")}),
        );
        println!("{}", serde_json::to_string(&row)?);
        write_json(output.join(format!("{}.json", arm.name)), &row)?;
        results.push(row);
    }
    let accepted = results.len() == manifest.arms.len()
        && results
            .iter()
            .all(|r| r["status"] == "passed" && r["committed_digest_matches"] == true);
    ensure!(
        sha(&fs::read(elf_path)?) == manifest.elf_sha256,
        "ELF changed during verification"
    );
    ensure!(
        fs::read(manifest_path)? == manifest_bytes,
        "manifest changed during verification"
    );
    let report = json!({
        "status": if accepted { "passed" } else { "failed" }, "mode": "manifest-core-verify",
        "run": manifest.run, "manifest_sha256": sha(&manifest_bytes), "elf_sha256": manifest.elf_sha256,
        "expected_guest_cycles": manifest.cycles, "expected_public_values_len": expected_bytes.len(),
        "expected_public_values_sha256": manifest.public_values_sha256,
        "verification_key_sha256": derived_vk_sha, "key_derivation_seconds": setup_seconds,
        "fri_config": "core", "log_stacking_height": EXPECTED_LOG_STACKING_HEIGHT,
        "max_log_row_count": EXPECTED_MAX_LOG_ROW_COUNT, "empty_proof_rejection_check": empty_error.check,
        "arms": results, "proof_generated": false, "gpu_used": false,
    });
    write_json(output.join("verification.json"), &report)?;
    ensure!(
        accepted,
        "one or more downloaded proofs failed local verification; see verification.json"
    );
    println!("LOCAL_CORE_VERIFICATION_PASSED");
    Ok(())
}

async fn run_verify(input: &Path, output: &Path) -> Result<()> {
    ensure!(
        input.is_dir(),
        "verification input must be a prove-mode directory"
    );
    ensure!(!output.exists(), "use a fresh output path");
    let verifier = ShardVerifier::from_basefold_parameters(
        FriConfig::default_fri_config(),
        LOG_STACKING_HEIGHT,
        MAX_LOG_ROW_COUNT,
        Air::machine(),
    );
    let cpu = Cpu::new(verifier.clone());
    let prover = SimpleProver::new(verifier, cpu);
    let mut results = Vec::new();
    let mut expected_total = 0usize;

    for op in &OPS {
        let dir = input.join(op.id);
        // A missing operation directory is a failure, never a skip.
        ensure!(
            dir.is_dir(),
            "missing operation directory {}: the inventory requires all four operations",
            dir.display()
        );
        let tag = OPS.iter().position(|o| o.id == op.id).unwrap() as u32;
        let policy = Policy {
            require_successful_exit: true,
            expected_committed_digest: Some(fixture_digest_bytes(tag)),
        };
        let vk_bytes = fs::read(dir.join("verification-key.bin"))
            .with_context(|| format!("{}: missing verification key", op.id))?;
        let vk: MachineVerifyingKey<SP1GlobalContext> = bincode::deserialize(&vk_bytes)?;

        // Program identity must not depend on any file in the artifact
        // directory. Re-deriving a key from an adjacent program.bin only proves
        // the bundle is self-consistent; the whole program/key/proof triple
        // could still be substituted. So rebuild the fixture program from this
        // binary's own code and require the saved program to equal it byte for
        // byte, then derive the key from the rebuilt program.
        let points = multiples(op.curve, 4);
        let expected_call = match op.kind {
            Kind::Add => make_call(*op, points[0].clone(), Some(points[1].clone()))?,
            Kind::Double => make_call(*op, points[0].clone(), None)?,
        };
        let (expected_program, _) = build_program_at(
            std::slice::from_ref(&expected_call),
            FIXTURE_BASE,
            true,
            false,
            0,
            Some(tag),
            0,
            0,
        )?;
        let expected_program_bytes = bincode::serialize(&*expected_program)?;
        let program_bytes = fs::read(dir.join("program.bin"))
            .with_context(|| format!("{}: missing program.bin", op.id))?;
        ensure!(
            program_bytes == expected_program_bytes,
            "{}: the saved program does not match the fixture this binary builds; \
             the artifact directory may hold a substituted program/key/proof bundle",
            op.id
        );
        // Derive the key from the independently rebuilt program, not from the
        // saved bytes, and require the stored key to match.
        let (_, derived_vk) = prover.setup(expected_program).await;
        let derived_bytes = bincode::serialize(&derived_vk)?;
        ensure!(
            derived_bytes == vk_bytes,
            "{}: the stored verification key does not match one derived from the \
             independently rebuilt fixture program",
            op.id
        );

        let expected: Vec<Case> = cases_for(op);
        expected_total += expected.len();
        // Every expected proof file must be present.
        let mut seen = std::collections::BTreeSet::new();
        for case in &expected {
            let path = dir.join(format!("{}.proof.bin", case.id()));
            ensure!(
                path.exists(),
                "missing proof {}: the inventory requires every case",
                path.display()
            );
            seen.insert(format!("{}.proof.bin", case.id()));
            let bytes = fs::read(&path)?;
            let proof: Proof = bincode::deserialize(&bytes)?;
            let complete = verify_complete_execution(&prover, &vk, &proof, &policy);
            let per_shard = prover.verify(&vk, &proof).is_ok();
            let accepted = complete.is_ok();
            let expected_acceptance = *case == Case::Honest;
            if let Some(expected_check) = case.expected_rejection_check() {
                ensure!(
                    complete.as_ref().err().map(|e| e.check) == Some(expected_check),
                    "{} {}: saved proof should be rejected at {expected_check}, got {:?}",
                    op.id,
                    case.id(),
                    complete.as_ref().err().map(|e| e.check)
                );
            }
            results.push(json!({
                "operation": op.id, "case": case.id(),
                "expected_rejection_check": case.expected_rejection_check(),
                "proof_path": path.to_string_lossy(),
                "proof_sha256": sha(&bytes),
                "verification_key_sha256": sha(&vk_bytes),
                "program_matches_independently_rebuilt_fixture": true,
                "verification_key_derived_from_rebuilt_program": true,
                "program_sha256": sha(&program_bytes),
                "serialized_shards": proof.shard_proofs.len(),
                "complete_verifier_accepted": accepted,
                "per_shard_verifier_accepted": per_shard,
                "expected_acceptance": expected_acceptance,
                "passed": accepted == expected_acceptance,
                "rejection_check": complete.as_ref().err().map(|e| e.check),
                "verifier_error": complete.as_ref().err().map(|e| e.detail.clone()),
            }));
            println!(
                "VERIFY {} {} complete_accepted={accepted} shards={}",
                op.id,
                case.id(),
                proof.shard_proofs.len()
            );
        }
        // Unexpected proof files mean the inventory and the directory disagree.
        for entry in fs::read_dir(&dir)? {
            let name = entry?.file_name().to_string_lossy().to_string();
            if name.ends_with(".proof.bin") {
                ensure!(
                    seen.contains(&name),
                    "{}: unexpected proof file {name} is not in the case inventory",
                    op.id
                );
            }
        }
    }

    ensure!(
        results.len() == expected_total,
        "verified {} proofs but the inventory requires {expected_total}",
        results.len()
    );
    let failed: Vec<&Value> = results
        .iter()
        .filter(|r| r["passed"] != json!(true))
        .collect();
    let summary = json!({
        "status": if failed.is_empty() { "passed" } else { "failed" },
        "mode": "verify-from-disk",
        "verification_endpoint": "complete-execution verifier (structure, policy, then per-shard cryptographic verification)",
        "program_identity_anchor": "each fixture program is rebuilt from this binary's own code and required to equal the saved program byte for byte; the verification key is then derived from the rebuilt program, so no file in the artifact directory is a trust anchor",
        "input": input.to_string_lossy(),
        "required_inventory": expected_total,
        "checked": results.len(),
        "results": results,
    });
    write_json(output, &summary)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    ensure!(failed.is_empty(), "saved-proof verification failed");
    Ok(())
}

/// Third independent inversion: extended Euclid rather than Fermat, so the
/// differential test compares three separately written implementations.
fn inv_egcd(a: &BigUint, m: &BigUint) -> BigUint {
    use num::bigint::{BigInt, Sign};
    let (mut old_r, mut r) = (
        BigInt::from_biguint(Sign::Plus, a.clone()),
        BigInt::from_biguint(Sign::Plus, m.clone()),
    );
    let (mut old_s, mut s) = (BigInt::from(1), BigInt::from(0));
    while r != BigInt::from(0) {
        let q = &old_r / &r;
        let new_r = &old_r - &q * &r;
        old_r = std::mem::replace(&mut r, new_r);
        let new_s = &old_s - &q * &s;
        old_s = std::mem::replace(&mut s, new_s);
    }
    assert_eq!(old_r, BigInt::from(1), "not invertible");
    let m_int = BigInt::from_biguint(Sign::Plus, m.clone());
    let normalized = ((old_s % &m_int) + &m_int) % &m_int;
    normalized
        .to_biguint()
        .expect("normalized inverse is non-negative")
}

fn egcd_add(p: &Point, q: &Point, m: &BigUint) -> Point {
    let slope = (sub_mod(&q.1, &p.1, m) * inv_egcd(&sub_mod(&q.0, &p.0, m), m)) % m;
    let x = sub_mod(&sub_mod(&((&slope * &slope) % m), &p.0, m), &q.0, m);
    let y = sub_mod(&((&slope * sub_mod(&p.0, &x, m)) % m), &p.1, m);
    (x, y)
}

fn egcd_double(p: &Point, m: &BigUint) -> Point {
    let two_y = (BigUint::from(2u32) * &p.1) % m;
    let slope = ((BigUint::from(3u32) * &p.0 * &p.0) % m * inv_egcd(&two_y, m)) % m;
    let x = sub_mod(&sub_mod(&((&slope * &slope) % m), &p.0, m), &p.0, m);
    let y = sub_mod(&((&slope * sub_mod(&p.0, &x, m)) % m), &p.1, m);
    (x, y)
}

/// Compare the independent Fermat reference, an independent extended-Euclid
/// reference, and the executor's own `sw_add`/`sw_double` over deterministic
/// multiples of the generator.
fn differential_tests(points_per_curve: usize) -> Result<Value> {
    use sp1_curves::AffinePoint;
    let mut report = Vec::new();
    let mut relations = 0usize;
    for curve in [Curve::Vesta, Curve::Pallas] {
        let m = modulus(curve);
        let points = multiples(curve, points_per_curve);
        let mut add_checked = 0usize;
        let mut double_checked = 0usize;
        for (i, p) in points.iter().enumerate() {
            ensure!(on_curve(p, &m), "generated multiple {i} is off curve");
            // Doubling: three implementations must agree.
            let fermat = ref_double(p, &m);
            let euclid = egcd_double(p, &m);
            ensure!(
                fermat == euclid,
                "{curve:?}: double references disagree at {i}"
            );
            let executor = match curve {
                Curve::Vesta => {
                    let a = AffinePoint::<Vesta>::new(p.0.clone(), p.1.clone()).sw_double();
                    (a.x, a.y)
                }
                Curve::Pallas => {
                    let a = AffinePoint::<Pallas>::new(p.0.clone(), p.1.clone()).sw_double();
                    (a.x, a.y)
                }
            };
            ensure!(
                fermat == executor,
                "{curve:?}: executor double disagrees at {i}"
            );
            ensure!(
                on_curve(&fermat, &m),
                "{curve:?}: double left the curve at {i}"
            );
            double_checked += 1;
            relations += 3;
            // Addition against every later point with a distinct x coordinate.
            for q in points.iter().skip(i + 1) {
                if p.0 == q.0 {
                    continue;
                }
                let fermat = ref_add(p, q, &m);
                let euclid = egcd_add(p, q, &m);
                ensure!(
                    fermat == euclid,
                    "{curve:?}: add references disagree at {i}"
                );
                let executor = match curve {
                    Curve::Vesta => {
                        let a = AffinePoint::<Vesta>::new(p.0.clone(), p.1.clone())
                            .sw_add(&AffinePoint::<Vesta>::new(q.0.clone(), q.1.clone()));
                        (a.x, a.y)
                    }
                    Curve::Pallas => {
                        let a = AffinePoint::<Pallas>::new(p.0.clone(), p.1.clone())
                            .sw_add(&AffinePoint::<Pallas>::new(q.0.clone(), q.1.clone()));
                        (a.x, a.y)
                    }
                };
                ensure!(
                    fermat == executor,
                    "{curve:?}: executor add disagrees at {i}"
                );
                ensure!(
                    on_curve(&fermat, &m),
                    "{curve:?}: add left the curve at {i}"
                );
                add_checked += 1;
                relations += 3;
            }
        }
        // Group-law sanity: the generator's multiples must satisfy kG + G = (k+1)G.
        let g = GroupPoint::Finite(generator(curve).0, generator(curve).1);
        for i in 0..points.len() - 1 {
            let next = group_add(
                &GroupPoint::Finite(points[i].0.clone(), points[i].1.clone()),
                &g,
                &m,
            );
            ensure!(
                next == GroupPoint::Finite(points[i + 1].0.clone(), points[i + 1].1.clone()),
                "{curve:?}: multiple chain inconsistent at {i}"
            );
            relations += 1;
        }
        report.push(json!({
            "curve": format!("{curve:?}"),
            "modulus_decimal": m.to_string(),
            "points": points.len(),
            "distinct_x_additions_checked": add_checked,
            "doublings_checked": double_checked,
        }));
    }
    Ok(json!({
        "implementations_compared": [
            "independent Fermat-inversion reference (this harness)",
            "independent extended-Euclid reference (this harness)",
            "sp1-curves sw_add / sw_double (the executor's own path)"
        ],
        "total_relations_checked": relations,
        "curves": report,
    }))
}

/// Integer witness-bound derivation for both Pasta base fields, following the
/// machine fork's documented argument. This is a supplemental arithmetic check,
/// not a proof test.
fn witness_bounds() -> Result<Value> {
    use num::bigint::{BigInt, Sign};
    const KOALABEAR_MODULUS: u64 = 0x7f00_0001;
    let mut curves = Vec::new();
    let mut operations = 0usize;
    for (name, nb_limbs, nb_witness, offset, m) in [
        (
            "VestaBaseField",
            VestaBaseField::NB_LIMBS,
            VestaBaseField::NB_WITNESS_LIMBS,
            VestaBaseField::WITNESS_OFFSET,
            VestaBaseField::modulus(),
        ),
        (
            "PallasBaseField",
            PallasBaseField::NB_LIMBS,
            PallasBaseField::NB_WITNESS_LIMBS,
            PallasBaseField::WITNESS_OFFSET,
            PallasBaseField::modulus(),
        ),
    ] {
        ensure!(
            nb_limbs == 32,
            "{name}: expected 32 byte limbs, got {nb_limbs}"
        );
        ensure!(
            nb_witness == 62,
            "{name}: expected 62 quotient coefficients, got {nb_witness}"
        );
        ensure!(
            offset == 1 << 14,
            "{name}: expected witness offset 16384, got {offset}"
        );

        let limbs = |v: &BigUint| -> Vec<BigInt> {
            (0..nb_limbs)
                .map(|i| {
                    BigInt::from(
                        ((v >> (8 * i)) & BigUint::from(0xffu32))
                            .to_u64_digits()
                            .first()
                            .copied()
                            .unwrap_or(0),
                    )
                })
                .collect()
        };
        let mul_poly = |a: &[BigInt], b: &[BigInt]| -> Vec<BigInt> {
            let mut out = vec![BigInt::from(0); a.len() + b.len() - 1];
            for (i, x) in a.iter().enumerate() {
                for (j, y) in b.iter().enumerate() {
                    out[i + j] += x * y;
                }
            }
            out
        };
        // Deterministic and boundary operands.
        let mut operands: Vec<BigUint> = vec![
            BigUint::zero(),
            BigUint::one(),
            BigUint::from(255u32),
            BigUint::from(256u32),
            &m - BigUint::one(),
            (&m - BigUint::one()) / BigUint::from(2u32),
            BigUint::from(1u32) << 255,
        ];
        for k in 1u32..=24 {
            operands.push((BigUint::from(k) * BigUint::from(0x0102_0304_0506_0708u64)) % &m);
        }
        let mut max_witness = BigInt::from(0);
        let mut encoded_min = u64::MAX;
        let mut encoded_max = 0u64;
        for a in &operands {
            for b in &operands {
                for op in ["add", "mul"] {
                    let (result, carry) = match op {
                        "add" => {
                            let sum = a + b;
                            let result = &sum % &m;
                            ((result.clone()), (&sum - &result) / &m)
                        }
                        _ => {
                            let product = a * b;
                            let result = &product % &m;
                            ((result.clone()), (&product - &result) / &m)
                        }
                    };
                    let (pa, pb) = (limbs(a), limbs(b));
                    let pr = limbs(&result);
                    let pc = limbs(&carry);
                    let pm = limbs(&m);
                    let lhs = if op == "add" {
                        let mut v = pa.clone();
                        for (i, x) in pb.iter().enumerate() {
                            v[i] += x;
                        }
                        v
                    } else {
                        mul_poly(&pa, &pb)
                    };
                    let km = mul_poly(&pc, &pm);
                    let degree = lhs.len().max(km.len()).max(pr.len());
                    let mut f = vec![BigInt::from(0); degree];
                    for (i, x) in lhs.iter().enumerate() {
                        f[i] += x;
                    }
                    for (i, x) in pr.iter().enumerate() {
                        f[i] -= x;
                    }
                    for (i, x) in km.iter().enumerate() {
                        f[i] -= x;
                    }
                    // The honest integer relation must vanish at X = 256.
                    let mut at_256 = BigInt::from(0);
                    for coefficient in f.iter().rev() {
                        at_256 = at_256 * BigInt::from(256) + coefficient;
                    }
                    ensure!(at_256 == BigInt::from(0), "{name}: f(256) != 0 for {op}");
                    // f(X) = (X - 256) w(X); recover w from the low end.
                    let mut w = Vec::with_capacity(f.len() - 1);
                    let mut carry_term = BigInt::from(0);
                    for i in 0..f.len() - 1 {
                        let value = (&carry_term - &f[i]) / BigInt::from(256);
                        ensure!(
                            (&carry_term - &f[i]) % BigInt::from(256) == BigInt::from(0),
                            "{name}: quotient recurrence is not exact"
                        );
                        w.push(value.clone());
                        carry_term = value;
                    }
                    ensure!(
                        w.len() <= nb_witness,
                        "{name}: {op} needs {} coefficients, only {nb_witness} are range-checked",
                        w.len()
                    );
                    for value in &w {
                        let magnitude = if value.sign() == Sign::Minus {
                            -value.clone()
                        } else {
                            value.clone()
                        };
                        if magnitude > max_witness {
                            max_witness = magnitude;
                        }
                        let encoded = value + BigInt::from(offset as u64);
                        ensure!(
                            encoded >= BigInt::from(0) && encoded <= BigInt::from(u16::MAX),
                            "{name}: encoded witness {encoded} escapes the range-checked 16-bit window"
                        );
                        let encoded = encoded.to_u64_digits().1.first().copied().unwrap_or(0);
                        encoded_min = encoded_min.min(encoded);
                        encoded_max = encoded_max.max(encoded);
                    }
                    operations += 1;
                }
            }
        }
        // Soundness margin: even an arbitrary range-checked witness decodes into
        // [-offset, 65535 - offset], and the residual then stays below the field.
        let coefficient_bound = 32u64 * 255 * 255 + 255;
        let residual_bound = coefficient_bound + 257 * (u16::MAX as u64 - offset as u64);
        ensure!(
            residual_bound < KOALABEAR_MODULUS,
            "{name}: residual bound {residual_bound} is not below the SP1 field modulus"
        );
        curves.push(json!({
            "field": name,
            "modulus_decimal": m.to_string(),
            "byte_limbs": nb_limbs,
            "quotient_witness_coefficients": nb_witness,
            "witness_offset": offset,
            "max_abs_witness_coefficient_observed": max_witness.to_string(),
            "derived_witness_bound": coefficient_bound / 255,
            "encoded_witness_min_observed": encoded_min,
            "encoded_witness_max_observed": encoded_max,
            "coefficient_bound": coefficient_bound,
            "residual_bound": residual_bound,
            "sp1_field_modulus": KOALABEAR_MODULUS,
        }));
    }
    Ok(json!({
        "integer_relations_checked": operations,
        "note": "supplemental integer check of the field-operation gadget's witness encoding; not an audit of SP1 memory, lookup or protocol constraints",
        "fields": curves,
    }))
}

/// Run honest witness construction on an input that violates a precondition and
/// report whether the host rejected it. A host rejection is NOT a proof-level
/// rejection and is reported as a separate outcome.
fn invalid_input_tests() -> Result<Value> {
    let mut results = Vec::new();
    let silence = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for curve in [Curve::Vesta, Curve::Pallas] {
        let m = modulus(curve);
        let pts = multiples(curve, 3);
        let p = pts[0].clone();
        let add_op = OPS
            .iter()
            .find(|o| o.curve == curve && o.kind == Kind::Add)
            .unwrap();
        let double_op = OPS
            .iter()
            .find(|o| o.curve == curve && o.kind == Kind::Double)
            .unwrap();
        let neg_p = (p.0.clone(), (&m - &p.1) % &m);
        let zero = (BigUint::zero(), BigUint::zero());
        // Derive the malformed second operands from 2G, so their x differs from
        // p's and the test isolates validation rather than the denominator.
        let other = pts[1].clone();
        let off_curve = (other.0.clone(), (&other.1 + BigUint::one()) % &m);
        let noncanonical = (&other.0 + &m, other.1.clone());
        let off_curve_p = (p.0.clone(), (&p.1 + BigUint::one()) % &m);
        let noncanonical_p = (&p.0 + &m, p.1.clone());
        let cases: Vec<(&str, &Op, Point, Option<Point>, &str)> = vec![
            ("add-equal-points", add_op, p.clone(), Some(p.clone()), "valid group-law exception: P + P; raw add requires distinct x"),
            ("add-opposite-points", add_op, p.clone(), Some(neg_p.clone()), "valid group-law exception: P + (-P) = identity; zero denominator with nonzero numerator"),
            ("add-aliased-pointers", add_op, p.clone(), Some(p.clone()), "aliasing policy: a0 and a1 describing the same point"),
            ("add-off-curve-q", add_op, p.clone(), Some(off_curve.clone()), "invalid encoding: q is not on the curve"),
            ("add-noncanonical-q", add_op, p.clone(), Some(noncanonical.clone()), "invalid encoding: q.x exceeds the base field modulus"),
            ("add-identity-encoding", add_op, p.clone(), Some(zero.clone()), "invalid encoding: there is no infinity representation in the raw ABI"),
            ("double-zero-y", double_op, (BigUint::zero(), BigUint::zero()), None, "zero denominator 2y with zero numerator 3x^2; off-curve input"),
            ("double-off-curve", double_op, off_curve_p.clone(), None, "invalid encoding: p is not on the curve"),
            ("double-noncanonical", double_op, noncanonical_p.clone(), None, "invalid encoding: p.x exceeds the base field modulus"),
        ];
        for (id, op, p_in, q_in, intent) in cases {
            let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<String> {
                let (program, _) = build_raw_program(op, &p_in, q_in.as_ref())?;
                let (records, _) =
                    generate_records::<SP1Field>(program, SP1Stdin::new(), core_opts(), [0; 4])?;
                let mut events = 0;
                for record in &records {
                    events += record.get_precompile_events(op.syscall).len();
                }
                if events == 0 {
                    return Ok("executed_without_precompile_event".into());
                }
                // Honest witness construction for the precompile row.
                let mut probe = records.clone();
                for record in probe.iter_mut() {
                    if record.get_precompile_events(op.syscall).is_empty() {
                        continue;
                    }
                    let _ = chip_trace_and_deps(record, op)?;
                }
                Ok("honest_witness_constructed".into())
            }));
            let (status, detail) = match outcome {
                Ok(Ok(state)) => (state, Value::Null),
                Ok(Err(error)) => ("host_error".to_string(), json!(error.to_string())),
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                        .unwrap_or_else(|| "non-string panic payload".into());
                    ("host_panic".to_string(), json!(message))
                }
            };
            // Pinned expectations. These outcomes are what the correctness
            // contract rests on — which inputs nothing below the guest rejects —
            // so a silent change must fail the suite rather than quietly
            // rewrite the report.
            let expected = match id {
                "add-equal-points"
                | "add-opposite-points"
                | "add-aliased-pointers"
                | "double-zero-y" => "host_panic",
                "add-off-curve-q"
                | "add-noncanonical-q"
                | "add-identity-encoding"
                | "double-off-curve"
                | "double-noncanonical" => "honest_witness_constructed",
                other => bail!("no pinned expectation for invalid-input case {other}"),
            };
            ensure!(
                status == expected,
                "{curve:?} {id}: expected honest witness construction to end in \
                 {expected}, observed {status}"
            );
            results.push(json!({
                "curve": format!("{curve:?}"),
                "case": id,
                "operation": op.id,
                "intent": intent,
                "outcome": status,
                "expected_outcome": expected,
                "detail": detail,
                "outcome_is_proof_level_rejection": false,
            }));
        }
    }
    std::panic::set_hook(silence);
    Ok(json!({
        "note": "these outcomes come from honest witness construction on the host; a host panic is NOT a cryptographic verifier rejection and is reported separately",
        "cases": results,
    }))
}

/// Build a program that performs one raw syscall with no output comparison, so
/// invalid inputs still reach the precompile.
fn build_raw_program(op: &Op, p: &Point, q: Option<&Point>) -> Result<(Arc<Program>, Value)> {
    let base = FIXTURE_BASE;
    let (p_addr, q_addr) = (base + P_OFFSET, base + Q_OFFSET);
    let mut instructions = Vec::new();
    load_address(&mut instructions, 10, p_addr);
    match op.kind {
        Kind::Add => load_address(&mut instructions, 11, q_addr),
        Kind::Double => instructions.push(Instruction::new(Opcode::ADDI, 11, 0, 0, false, true)),
    }
    let syscall = op.syscall as u32 as u64;
    instructions.push(Instruction::new(
        Opcode::LUI,
        5,
        syscall & !0xfff,
        syscall & !0xfff,
        true,
        true,
    ));
    instructions.push(Instruction::new(
        Opcode::ADDI,
        5,
        5,
        syscall & 0xfff,
        false,
        true,
    ));
    instructions.push(Instruction::new(Opcode::ECALL, 5, 10, 11, false, false));
    add_halt(&mut instructions);
    let mut program = Program::new(instructions, 0x2000, 0x2000);
    let image = Arc::make_mut(&mut program.memory_image);
    for (i, word) in words(p).into_iter().enumerate() {
        image.insert(p_addr + 8 * i as u64, word);
    }
    if let Some(q) = q {
        for (i, word) in words(q).into_iter().enumerate() {
            image.insert(q_addr + 8 * i as u64, word);
        }
    }
    Ok((Arc::new(program), json!({"raw": true})))
}

/// Evaluate the real chip AIR against honest and tampered rows with
/// `debug_constraints`, which checks the algebraic constraints only (the
/// lookup/interaction arguments are a separate mechanism).
fn constraint_level_tests() -> Result<Value> {
    let mut results = Vec::new();
    for op in &OPS {
        let points = multiples(op.curve, 2);
        let call = match op.kind {
            Kind::Add => make_call(*op, points[0].clone(), Some(points[1].clone()))?,
            Kind::Double => make_call(*op, points[0].clone(), None)?,
        };
        let (program, _) = build_program(std::slice::from_ref(&call))?;
        let (records, _) =
            generate_records::<SP1Field>(program, SP1Stdin::new(), core_opts(), [0; 4])?;
        let machine = Air::machine();
        let chip = machine
            .chips()
            .iter()
            .find(|c| c.name() == op.chip)
            .context("chip not found in machine")?;
        let mut done = false;
        for record in &records {
            if record.get_precompile_events(op.syscall).is_empty() {
                continue;
            }
            let mut input = record.clone();
            let trace = generate_chip_trace(&mut input, op);
            let width = op_width(op);
            ensure!(
                trace.values.len() >= width,
                "generated trace is shorter than one row"
            );

            // Control: the honest row must satisfy every algebraic constraint.
            let honest_failures = eval_rows(chip, trace.clone());
            // Enforced, not merely observed.
            ensure!(
                honest_failures.is_empty(),
                "{}: the honest row must satisfy every algebraic constraint, {} rows failed",
                op.id,
                honest_failures.len()
            );
            results.push(json!({
                "operation": op.id,
                "case": "honest-row",
                "expectation": "no failing algebraic constraints",
                "failing_rows": honest_failures.len(),
                "passed": honest_failures.is_empty(),
            }));

            // Tamper: force q.x = p.x (add) or p.y = 0 (double) in the row's
            // input columns and report which constraints then fail.
            let mut tampered = trace.clone();
            let label = match op.kind {
                Kind::Add => {
                    force_equal_x(&mut tampered.values[..width], op)?;
                    "add-row-with-equal-x"
                }
                Kind::Double => {
                    force_zero_y(&mut tampered.values[..width], op)?;
                    "double-row-with-zero-y"
                }
            };
            let failures = eval_rows(chip, tampered);
            ensure!(
                !failures.is_empty(),
                "{}: tampering the row's input columns must break the algebraic constraints",
                op.id
            );
            let constraint_indices: Vec<usize> = failures
                .first()
                .map(|(_, c, _)| c.clone())
                .unwrap_or_default();
            results.push(json!({
                "operation": op.id,
                "case": label,
                "expectation": match op.kind {
                    Kind::Add => "the add chip's inverse_check and denominator constraints must fail, so q.x == p.x cannot be witnessed",
                    Kind::Double => "the double chip has no inverse_check column; the denominator relation alone governs 2y == 0",
                },
                "failing_rows": failures.len(),
                "first_failing_constraint_indices": constraint_indices,
                "air_rejects_row": !failures.is_empty(),
            }));
            done = true;
            break;
        }
        ensure!(done, "{}: no shard carried the precompile event", op.id);
    }
    Ok(json!({
        "method": "sp1_hypercube::debug_constraints on the real chip AIR",
        "scope": "algebraic row constraints only; the memory and syscall interaction arguments are checked by the proof tests instead",
        "results": results,
    }))
}

fn generate_chip_trace(
    record: &mut ExecutionRecord,
    op: &Op,
) -> slop_matrix::dense::RowMajorMatrix<SP1Field> {
    let mut discard = ExecutionRecord::default();
    match (op.curve, op.kind) {
        (Curve::Vesta, Kind::Add) => MachineAir::<SP1Field>::generate_trace(
            &WeierstrassAddAssignChip::<Vesta, sp1_core_machine::SupervisorMode>::new(),
            record,
            &mut discard,
        ),
        (Curve::Pallas, Kind::Add) => MachineAir::<SP1Field>::generate_trace(
            &WeierstrassAddAssignChip::<Pallas, sp1_core_machine::SupervisorMode>::new(),
            record,
            &mut discard,
        ),
        (Curve::Vesta, Kind::Double) => MachineAir::<SP1Field>::generate_trace(
            &WeierstrassDoubleAssignChip::<Vesta, sp1_core_machine::SupervisorMode>::new(),
            record,
            &mut discard,
        ),
        (Curve::Pallas, Kind::Double) => MachineAir::<SP1Field>::generate_trace(
            &WeierstrassDoubleAssignChip::<Pallas, sp1_core_machine::SupervisorMode>::new(),
            record,
            &mut discard,
        ),
    }
}

fn eval_rows(
    chip: &Chip<SP1Field, Air>,
    trace: slop_matrix::dense::RowMajorMatrix<SP1Field>,
) -> Vec<(usize, Vec<usize>, Vec<SP1Field>)> {
    let mle: Mle<SP1Field> = Mle::from(trace);
    debug_constraints::<SP1GlobalContext, Air>(chip, None, &mle, &[])
}

/// Overwrite the row's q x-coordinate byte limbs with p's, so the row claims
/// two points with the same x coordinate.
fn force_equal_x(row: &mut [SP1Field], op: &Op) -> Result<()> {
    macro_rules! apply {
        ($ty:ty) => {{
            let cols: &mut $ty = row.borrow_mut();
            let half = cols.p_access.len() / 2;
            for i in 0..half {
                cols.q_access[i].prev_value_u8 = cols.p_access[i].prev_value_u8;
                cols.q_access[i].memory_access = cols.p_access[i].memory_access;
            }
        }};
    }
    match op.curve {
        Curve::Vesta => {
            apply!(WeierstrassAddAssignCols<SP1Field, VestaBaseField, sp1_core_machine::SupervisorMode>)
        }
        Curve::Pallas => {
            apply!(WeierstrassAddAssignCols<SP1Field, PallasBaseField, sp1_core_machine::SupervisorMode>)
        }
    }
    Ok(())
}

/// Overwrite the row's y-coordinate byte limbs with zero, so the row claims a
/// point with 2y == 0.
fn force_zero_y(row: &mut [SP1Field], op: &Op) -> Result<()> {
    macro_rules! apply {
        ($ty:ty) => {{
            let cols: &mut $ty = row.borrow_mut();
            let half = cols.p_access.len() / 2;
            for i in half..cols.p_access.len() {
                cols.p_access[i].prev_value_u8 = Default::default();
            }
        }};
    }
    match op.curve {
        Curve::Vesta => {
            apply!(WeierstrassDoubleAssignCols<SP1Field, VestaBaseField, sp1_core_machine::SupervisorMode>)
        }
        Curve::Pallas => {
            apply!(WeierstrassDoubleAssignCols<SP1Field, PallasBaseField, sp1_core_machine::SupervisorMode>)
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Decisive degenerate-witness test.
//
// The two chips differ in exactly one column. The add chip carries an
// `inverse_check` column constrained by `inverse_check * (q.x - p.x) == 1`, so
// a zero denominator has no satisfying witness. The double chip has no such
// column: its only link between the slope and the input is
// `slope * 2y == 3x^2 + a`. When the claimed input is the off-curve encoding
// (0, 0) both sides vanish and the slope is unconstrained.
//
// Rather than argue that from the source, this builds a *fully self-consistent*
// forged row for each chip: the input columns claim the degenerate point and
// every dependent field-operation column is repopulated to match a chosen
// slope, using `populate_carry_and_witness` for the division columns whose
// denominator is zero. `debug_constraints` then reports whether the real AIR
// accepts the row.
// ---------------------------------------------------------------------------

fn set_input_word<P: FieldParameters + NumWords>(
    access: &mut sp1_core_machine::memory::MemoryAccessColsU8<SP1Field>,
    value: u64,
) {
    access.memory_access.prev_value = sp1_hypercube::Word::from(value);
    access.prev_value_u8.populate_u16_to_u8_unsafe(value);
    let _ = core::marker::PhantomData::<P>;
}

/// Repopulate the double chip's arithmetic for input (0, 0) with a chosen slope.
fn forge_double_row<P: FieldParameters + NumWords>(
    row: &mut [SP1Field],
    slope: &BigUint,
) -> Result<([u64; 4], [u64; 4])> {
    use sp1_core_executor::events::FieldOperation;
    let cols: &mut WeierstrassDoubleAssignCols<SP1Field, P, sp1_core_machine::SupervisorMode> =
        row.borrow_mut();
    let m = P::modulus();
    let zero = BigUint::zero();
    let mut blu = Vec::new();

    // Claim the all-zero input encoding in the memory-value columns.
    for i in 0..cols.p_access.len() {
        set_input_word::<P>(&mut cols.p_access[i], 0);
    }

    // slope_numerator = a + 3 * x^2 = 0 for x = 0 and a = 0.
    let x_squared = cols
        .p_x_squared
        .populate(&mut blu, &zero, &zero, FieldOperation::Mul);
    let x_squared_times_3 = cols.p_x_squared_times_3.populate(
        &mut blu,
        &x_squared,
        &BigUint::from(3u32),
        FieldOperation::Mul,
    );
    let numerator =
        cols.slope_numerator
            .populate(&mut blu, &zero, &x_squared_times_3, FieldOperation::Add);
    ensure!(numerator.is_zero(), "forged numerator should vanish");

    // slope_denominator = 2 * y = 0 for y = 0.
    let denominator =
        cols.slope_denominator
            .populate(&mut blu, &BigUint::from(2u32), &zero, FieldOperation::Mul);
    ensure!(denominator.is_zero(), "forged denominator should vanish");

    // The division column: `slope * denominator == numerator` is 0 == 0 for any
    // slope, so populate the reversed multiplication witness by hand. The
    // library's `populate` refuses to divide by zero, which is exactly the
    // honest-prover guard a malicious prover would skip.
    cols.slope
        .populate_carry_and_witness(slope, &denominator, FieldOperation::Mul, &m);
    cols.slope.result = P::to_limbs_field::<SP1Field, SP1Field>(slope);

    // Everything downstream follows from the forged slope.
    let slope_squared = cols
        .slope_squared
        .populate(&mut blu, slope, slope, FieldOperation::Mul);
    let x_plus_x = cols
        .p_x_plus_p_x
        .populate(&mut blu, &zero, &zero, FieldOperation::Add);
    let x3 = cols
        .x3_ins
        .populate(&mut blu, &slope_squared, &x_plus_x, FieldOperation::Sub);
    cols.x3_range.populate(&mut blu, &x3, &m);
    let x_minus_x3 = cols
        .p_x_minus_x
        .populate(&mut blu, &zero, &x3, FieldOperation::Sub);
    let product =
        cols.slope_times_p_x_minus_x
            .populate(&mut blu, slope, &x_minus_x3, FieldOperation::Mul);
    let y3 = cols
        .y3_ins
        .populate(&mut blu, &product, &zero, FieldOperation::Sub);
    cols.y3_range.populate(&mut blu, &y3, &m);

    Ok((biguint_to_words(&x3), biguint_to_words(&y3)))
}

/// Repopulate the add chip's arithmetic for q == p with a chosen slope.
fn forge_add_equal_points_row<P: FieldParameters + NumWords>(
    row: &mut [SP1Field],
    slope: &BigUint,
) -> Result<()> {
    use sp1_core_executor::events::FieldOperation;
    let cols: &mut WeierstrassAddAssignCols<SP1Field, P, sp1_core_machine::SupervisorMode> =
        row.borrow_mut();
    let m = P::modulus();
    let zero = BigUint::zero();
    let one = BigUint::one();
    let mut blu = Vec::new();

    // Copy p's claimed input words into q's, so the row describes q == p.
    for i in 0..cols.p_access.len() {
        cols.q_access[i].memory_access.prev_value = cols.p_access[i].memory_access.prev_value;
        cols.q_access[i].prev_value_u8 = cols.p_access[i].prev_value_u8;
    }
    // Recover p's coordinates from the (unchanged) input columns.
    let half = cols.p_access.len() / 2;
    let mut x_words = [0u64; 4];
    let mut y_words = [0u64; 4];
    for i in 0..half {
        x_words[i] = word_to_u64(&cols.p_access[i].memory_access.prev_value);
        y_words[i] = word_to_u64(&cols.p_access[half + i].memory_access.prev_value);
    }
    let (p_x, p_y) = point_from_words(&[
        x_words[0], x_words[1], x_words[2], x_words[3], y_words[0], y_words[1], y_words[2],
        y_words[3],
    ]);

    // Both the numerator and the denominator vanish when q == p.
    let denominator = cols
        .slope_denominator
        .populate(&mut blu, &p_x, &p_x, FieldOperation::Sub);
    ensure!(
        denominator.is_zero(),
        "forged add denominator should vanish"
    );
    let numerator = cols
        .slope_numerator
        .populate(&mut blu, &p_y, &p_y, FieldOperation::Sub);
    ensure!(numerator.is_zero(), "forged add numerator should vanish");

    // The add chip's nonzero-denominator column: `inverse_check * 0 == 1` has no
    // solution, but populate the best-effort witness so the row is otherwise
    // well formed and the failure is attributable to this constraint alone.
    cols.inverse_check
        .populate_carry_and_witness(&one, &denominator, FieldOperation::Mul, &m);
    cols.inverse_check.result = P::to_limbs_field::<SP1Field, SP1Field>(&one);

    cols.slope
        .populate_carry_and_witness(slope, &denominator, FieldOperation::Mul, &m);
    cols.slope.result = P::to_limbs_field::<SP1Field, SP1Field>(slope);

    let slope_squared = cols
        .slope_squared
        .populate(&mut blu, slope, slope, FieldOperation::Mul);
    let x_plus_x = cols
        .p_x_plus_q_x
        .populate(&mut blu, &p_x, &p_x, FieldOperation::Add);
    let x3 = cols
        .x3_ins
        .populate(&mut blu, &slope_squared, &x_plus_x, FieldOperation::Sub);
    cols.x3_range.populate(&mut blu, &x3, &m);
    let x_minus_x3 = cols
        .p_x_minus_x
        .populate(&mut blu, &p_x, &x3, FieldOperation::Sub);
    let product =
        cols.slope_times_p_x_minus_x
            .populate(&mut blu, slope, &x_minus_x3, FieldOperation::Mul);
    let y3 = cols
        .y3_ins
        .populate(&mut blu, &product, &p_y, FieldOperation::Sub);
    cols.y3_range.populate(&mut blu, &y3, &m);
    ensure!(zero.is_zero(), "unreachable");
    Ok(())
}

fn biguint_to_words(value: &BigUint) -> [u64; 4] {
    let mut out = [0u64; 4];
    let digits = value.to_u64_digits();
    out[..digits.len()].copy_from_slice(&digits);
    out
}

fn word_to_u64(word: &sp1_hypercube::Word<SP1Field>) -> u64 {
    let mut out = 0u64;
    for (i, limb) in word.0.iter().enumerate() {
        out |= (limb.as_canonical_u32() as u64) << (16 * i);
    }
    out
}

/// Build the forged rows and report what the real AIR does with them.
fn degenerate_witness_tests() -> Result<Value> {
    let mut results = Vec::new();
    for op in &OPS {
        let points = multiples(op.curve, 2);
        let call = match op.kind {
            Kind::Add => make_call(*op, points[0].clone(), Some(points[1].clone()))?,
            Kind::Double => make_call(*op, points[0].clone(), None)?,
        };
        let (program, _) = build_program(std::slice::from_ref(&call))?;
        let (records, _) =
            generate_records::<SP1Field>(program, SP1Stdin::new(), core_opts(), [0; 4])?;
        let machine = Air::machine();
        let chip = machine
            .chips()
            .iter()
            .find(|c| c.name() == op.chip)
            .context("chip not in machine")?;
        let width = op_width(op);
        let record = records
            .iter()
            .find(|r| !r.get_precompile_events(op.syscall).is_empty())
            .context("no shard carried the precompile event")?;
        let mut input = record.clone();
        let honest = generate_chip_trace(&mut input, op);

        // Two different forged slopes. If the AIR accepts both, it admits two
        // distinct outputs for one claimed input: the relation is not binding.
        let slopes = [BigUint::from(7u32), BigUint::from(123_456_789u32)];
        let mut accepted = Vec::new();
        for slope in &slopes {
            let mut trace = honest.clone();
            let outcome = match (op.curve, op.kind) {
                (Curve::Vesta, Kind::Double) => {
                    forge_double_row::<VestaBaseField>(&mut trace.values[..width], slope)
                        .map(|(x, y)| json!({"forged_x3_words_le": x, "forged_y3_words_le": y}))
                }
                (Curve::Pallas, Kind::Double) => {
                    forge_double_row::<PallasBaseField>(&mut trace.values[..width], slope)
                        .map(|(x, y)| json!({"forged_x3_words_le": x, "forged_y3_words_le": y}))
                }
                (Curve::Vesta, Kind::Add) => {
                    forge_add_equal_points_row::<VestaBaseField>(&mut trace.values[..width], slope)
                        .map(|_| Value::Null)
                }
                (Curve::Pallas, Kind::Add) => {
                    forge_add_equal_points_row::<PallasBaseField>(&mut trace.values[..width], slope)
                        .map(|_| Value::Null)
                }
            }?;
            let failures = eval_rows(chip, trace);
            let failing_constraints: Vec<usize> = failures
                .first()
                .map(|(_, c, _)| c.clone())
                .unwrap_or_default();
            accepted.push(json!({
                "forged_slope": slope.to_string(),
                "failing_rows": failures.len(),
                "air_accepts_forged_row": failures.is_empty(),
                "failing_constraint_indices": failing_constraints,
                "forged_output": outcome,
            }));
        }
        let all_accepted = accepted
            .iter()
            .all(|a| a["air_accepts_forged_row"] == json!(true));
        let distinct_outputs =
            op.kind == Kind::Double && accepted[0]["forged_output"] != accepted[1]["forged_output"];
        // These are enforced expectations. The add chip carries an
        // `inverse_check` column, so a zero denominator has no witness; the
        // double chip does not, so the slope is free for the (0,0) input.
        match op.kind {
            Kind::Add => {
                ensure!(
                    !all_accepted,
                    "{}: the add AIR must reject a self-consistent q == p row",
                    op.id
                );
                for attempt in &accepted {
                    ensure!(
                        attempt["failing_rows"].as_u64() == Some(1),
                        "{}: expected exactly one failing row for the forged add row",
                        op.id
                    );
                }
            }
            Kind::Double => {
                ensure!(
                    all_accepted,
                    "{}: the double AIR is expected to accept a self-consistent (0,0) row; \
                     if this now fails the documented contract has changed",
                    op.id
                );
                ensure!(
                    distinct_outputs,
                    "{}: two forged slopes must yield different claimed outputs",
                    op.id
                );
            }
        }
        results.push(json!({
            "operation": op.id,
            "chip": op.chip,
            "degenerate_input": match op.kind {
                Kind::Add => "q == p, so both the slope numerator and denominator vanish",
                Kind::Double => "the off-curve encoding (0, 0), so both 3x^2 and 2y vanish",
            },
            "has_inverse_check_column": op.kind == Kind::Add,
            "attempts": accepted,
            "air_accepts_a_self_consistent_degenerate_witness": all_accepted,
            "two_distinct_slopes_both_accepted_with_different_outputs": distinct_outputs,
            "conclusion": if all_accepted {
                "the chip's algebraic constraints do NOT pin the slope for this input; the precondition must be proved by the guest"
            } else {
                "the chip's algebraic constraints reject this input, so no self-consistent witness exists"
            },
        }));
    }
    Ok(json!({
        "method": "self-consistent forged rows evaluated with sp1_hypercube::debug_constraints on the real chip AIR",
        "scope": "algebraic row constraints. A row the AIR accepts here would still need consistent memory and syscall interactions to become a full proof; this suite did not construct that program.",
        "results": results,
    }))
}

/// Focused per-equation corruption tests.
///
/// A trace mutation in the proof matrix cannot have its byte/range lookup
/// multiplicities repaired: those tables are built from the record before the
/// trace is mutated, so the resulting GKR cumulative-sum failure cannot be
/// attributed to one arithmetic equation. These tests sidestep the problem by
/// evaluating the chip's algebraic constraints directly with
/// `debug_constraints`, which does not involve the lookup argument at all, so
/// each corruption is attributable to the constraints it breaks.
fn focused_equation_tests() -> Result<Value> {
    let mut results = Vec::new();
    for op in &OPS {
        let points = multiples(op.curve, 2);
        let call = match op.kind {
            Kind::Add => make_call(*op, points[0].clone(), Some(points[1].clone()))?,
            Kind::Double => make_call(*op, points[0].clone(), None)?,
        };
        let (program, _) = build_program(std::slice::from_ref(&call))?;
        let (records, _) =
            generate_records::<SP1Field>(program, SP1Stdin::new(), core_opts(), [0; 4])?;
        let machine = Air::machine();
        let chip = machine
            .chips()
            .iter()
            .find(|c| c.name() == op.chip)
            .context("chip not in machine")?;
        let width = op_width(op);
        let record = records
            .iter()
            .find(|r| !r.get_precompile_events(op.syscall).is_empty())
            .context("no shard carried the precompile event")?;
        let mut input = record.clone();
        let honest = generate_chip_trace(&mut input, op);

        let mut targets: Vec<Case> = vec![
            Case::ResultX3,
            Case::ResultY3,
            Case::Slope,
            Case::SlopeDenominator,
        ];
        if op.kind == Kind::Add {
            targets.push(Case::InverseCheck);
        }
        for case in targets {
            let mut trace = honest.clone();
            let mutation = mutate_row_limb(&mut trace.values[..width], op, case)?;
            let failures = eval_rows(chip, trace);
            let indices: Vec<usize> = failures
                .first()
                .map(|(_, c, _)| c.clone())
                .unwrap_or_default();
            ensure!(
                !failures.is_empty(),
                "{} {}: corrupting this column must break the algebraic constraints",
                op.id,
                case.id()
            );
            results.push(json!({
                "operation": op.id,
                "target": trace_target_name(case),
                "mutation": mutation,
                "failing_rows": failures.len(),
                "failing_constraint_indices": indices,
                "lookup_argument_involved": false,
                "passed": true,
            }));
        }
    }
    Ok(json!({
        "method": "sp1_hypercube::debug_constraints on the real chip AIR, one corrupted column at a time",
        "why": "the proof matrix's trace mutations leave stale byte/range multiplicities, so their generic GKR failure is not attributable to one equation; these checks involve no lookup argument and therefore are",
        "results": results,
    }))
}

/// Corrupt one byte limb of a named arithmetic column in a row, returning the
/// before/after evidence. Shared by the proof-matrix trace mutation and the
/// focused equation tests.
fn mutate_row_limb(row: &mut [SP1Field], op: &Op, case: Case) -> Result<Value> {
    macro_rules! pick {
        ($ty:ty, $with_inverse:expr) => {{
            let cols: &mut $ty = row.borrow_mut();
            match case {
                Case::ResultX3 => &mut cols.x3_ins.result.0[0],
                Case::ResultY3 => &mut cols.y3_ins.result.0[0],
                Case::Slope => &mut cols.slope.result.0[0],
                Case::SlopeDenominator => &mut cols.slope_denominator.result.0[0],
                Case::InverseCheck if $with_inverse => {
                    bail!("inverse_check handled separately")
                }
                other => bail!("{:?} is not a row-limb mutation", other),
            }
        }};
    }
    let limb: &mut SP1Field = match (op.curve, op.kind, case) {
        (Curve::Vesta, Kind::Add, Case::InverseCheck) => {
            let cols: &mut WeierstrassAddAssignCols<
                SP1Field,
                VestaBaseField,
                sp1_core_machine::SupervisorMode,
            > = row.borrow_mut();
            &mut cols.inverse_check.result.0[0]
        }
        (Curve::Pallas, Kind::Add, Case::InverseCheck) => {
            let cols: &mut WeierstrassAddAssignCols<
                SP1Field,
                PallasBaseField,
                sp1_core_machine::SupervisorMode,
            > = row.borrow_mut();
            &mut cols.inverse_check.result.0[0]
        }
        (Curve::Vesta, Kind::Add, _) => {
            pick!(WeierstrassAddAssignCols<SP1Field, VestaBaseField, sp1_core_machine::SupervisorMode>, false)
        }
        (Curve::Pallas, Kind::Add, _) => {
            pick!(WeierstrassAddAssignCols<SP1Field, PallasBaseField, sp1_core_machine::SupervisorMode>, false)
        }
        (Curve::Vesta, Kind::Double, _) => {
            pick!(WeierstrassDoubleAssignCols<SP1Field, VestaBaseField, sp1_core_machine::SupervisorMode>, false)
        }
        (Curve::Pallas, Kind::Double, _) => {
            pick!(WeierstrassDoubleAssignCols<SP1Field, PallasBaseField, sp1_core_machine::SupervisorMode>, false)
        }
    };
    let before = limb.as_canonical_u32();
    ensure!(before < 256, "expected a byte limb, got {before}");
    let after = before ^ 1;
    *limb = SP1Field::from_canonical_u32(after);
    Ok(json!({"before": before, "after": after}))
}

fn run_analysis(output: &Path) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    fs::create_dir_all(output)?;
    let started = Instant::now();
    let registration = registration_report()?;
    write_json(output.join("registration.json"), &registration)?;
    println!("ANALYSIS differential");
    let differential = differential_tests(12)?;
    write_json(output.join("differential.json"), &differential)?;
    println!("ANALYSIS witness-bounds");
    let bounds = witness_bounds()?;
    write_json(output.join("witness-bounds.json"), &bounds)?;
    println!("ANALYSIS invalid-inputs");
    let invalid = invalid_input_tests()?;
    write_json(output.join("invalid-inputs.json"), &invalid)?;
    println!("ANALYSIS constraint-level");
    let constraints = constraint_level_tests()?;
    write_json(output.join("constraint-level.json"), &constraints)?;
    println!("ANALYSIS focused-equations");
    let focused = focused_equation_tests()?;
    write_json(output.join("focused-equations.json"), &focused)?;
    println!("ANALYSIS degenerate-witness");
    let degenerate = degenerate_witness_tests()?;
    write_json(output.join("degenerate-witness.json"), &degenerate)?;
    // Every sub-check above returns Err on an unmet expectation, so reaching
    // this point means all of them were asserted, not merely recorded.
    let summary = json!({
        "status": "passed",
        "assertions_enforced": true,
        "mode": "analysis",
        "note": "host-only analysis; no cryptographic proof is produced or claimed in this mode",
        "registration": registration,
        "differential": differential,
        "witness_bounds": bounds,
        "invalid_inputs": invalid,
        "constraint_level": constraints,
        "focused_equations": focused,
        "degenerate_witness": degenerate,
        "total_seconds": started.elapsed().as_secs_f64(),
    });
    write_json(output.join("result.json"), &summary)?;
    println!("ANALYSIS_PASSED");
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Mode: guest (compiled SP1 ELF using all four wrappers)
// ---------------------------------------------------------------------------

const GUEST_SCALAR: [u64; 4] = [0xdead_beef, 0, 0, 0];
const GUEST_POINTS_PER_CURVE: usize = 3;

/// Host mirror of the guest's complete dispatch, written against the
/// independent reference arithmetic. It recomputes the exact byte stream the
/// guest must commit, including the precompile call counts, so the proof's
/// committed public-value digest can be checked against it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct MirrorPoint {
    coordinates: [u64; 8],
    infinity: bool,
}

#[derive(Default, Clone, Copy, Debug)]
struct MirrorCounts {
    adds: u32,
    doubles: u32,
}

impl MirrorPoint {
    const IDENTITY: Self = Self {
        coordinates: [0; 8],
        infinity: true,
    };

    fn finite(p: &Point) -> Self {
        Self {
            coordinates: words(p),
            infinity: false,
        }
    }

    fn point(&self) -> Point {
        point_from_words(&self.coordinates)
    }

    fn negated(&self, m: &BigUint) -> Self {
        if self.infinity {
            return Self::IDENTITY;
        }
        let p = self.point();
        if p.1.is_zero() {
            return *self;
        }
        Self::finite(&(p.0, m - p.1))
    }

    fn add_assign(&mut self, rhs: &Self, m: &BigUint, counts: &mut MirrorCounts) {
        if rhs.infinity {
            return;
        }
        if self.infinity {
            *self = *rhs;
            return;
        }
        if self.coordinates[..4] == rhs.coordinates[..4] {
            if self.coordinates[4..] == rhs.coordinates[4..] {
                self.double_assign(m, counts);
            } else {
                assert_eq!(
                    self.coordinates[4..],
                    rhs.negated(m).coordinates[4..],
                    "equal-x points are not opposites"
                );
                *self = Self::IDENTITY;
            }
            return;
        }
        counts.adds += 1;
        *self = Self::finite(&ref_add(&self.point(), &rhs.point(), m));
    }

    fn double_assign(&mut self, m: &BigUint, counts: &mut MirrorCounts) {
        if self.infinity {
            return;
        }
        if self.coordinates[4..] == [0; 4] {
            *self = Self::IDENTITY;
            return;
        }
        counts.doubles += 1;
        *self = Self::finite(&ref_double(&self.point(), m));
    }

    fn multiply(&self, scalar: &[u64; 4], m: &BigUint, counts: &mut MirrorCounts) -> Self {
        let mut accumulator = Self::IDENTITY;
        let mut started = false;
        for i in (0..4).rev() {
            for bit in (0..64).rev() {
                if started {
                    accumulator.double_assign(m, counts);
                }
                if (scalar[i] >> bit) & 1 == 1 {
                    accumulator.add_assign(self, m, counts);
                    started = true;
                }
            }
        }
        accumulator
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(self.infinity));
        for limb in self.coordinates {
            out.extend_from_slice(&limb.to_le_bytes());
        }
    }
}

fn guest_input_blob(points: &[(Curve, Vec<Point>)]) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(b"PST1");
    blob.push(points[0].1.len() as u8);
    blob.push(points[1].1.len() as u8);
    for limb in GUEST_SCALAR {
        blob.extend_from_slice(&limb.to_le_bytes());
    }
    for (_, list) in points {
        for p in list {
            for limb in words(p) {
                blob.extend_from_slice(&limb.to_le_bytes());
            }
        }
    }
    blob
}

/// Recompute the guest's committed byte stream independently.
fn expected_commitment(points: &[(Curve, Vec<Point>)], input: &[u8]) -> Result<(Vec<u8>, Value)> {
    let mut committed = Vec::new();
    committed.extend_from_slice(b"PSO1");
    committed.extend_from_slice(input);
    let mut totals = MirrorCounts::default();
    let mut detail = Vec::new();
    for (curve, list) in points {
        let m = modulus(*curve);
        let mut counts = MirrorCounts::default();
        let mirrored: Vec<MirrorPoint> = list.iter().map(MirrorPoint::finite).collect();

        let mut sum = MirrorPoint::IDENTITY;
        for p in &mirrored {
            sum.add_assign(p, &m, &mut counts);
        }
        let mut doubled = mirrored[0];
        doubled.double_assign(&m, &mut counts);
        let scaled = mirrored[0].multiply(&GUEST_SCALAR, &m, &mut counts);

        let mut check = mirrored[0];
        check.add_assign(&mirrored[0], &m, &mut counts);
        ensure!(check == doubled, "mirror: P + P disagreed with 2P");
        let mut opposite = mirrored[0];
        opposite.add_assign(&mirrored[0].negated(&m), &m, &mut counts);
        ensure!(opposite.infinity, "mirror: P + (-P) was not the identity");

        // Cross-check the scalar multiple against the complete group law.
        let mut group_acc = GroupPoint::Infinity;
        let base = GroupPoint::Finite(list[0].0.clone(), list[0].1.clone());
        let mut started = false;
        for i in (0..4).rev() {
            for bit in (0..64).rev() {
                if started {
                    group_acc = group_add(&group_acc.clone(), &group_acc, &m);
                }
                if (GUEST_SCALAR[i] >> bit) & 1 == 1 {
                    group_acc = group_add(&group_acc, &base, &m);
                    started = true;
                }
            }
        }
        let group_scaled = match group_acc {
            GroupPoint::Finite(x, y) => MirrorPoint::finite(&(x, y)),
            GroupPoint::Infinity => MirrorPoint::IDENTITY,
        };
        ensure!(
            group_scaled == scaled,
            "mirror: double-and-add disagreed with the complete group law"
        );

        sum.encode(&mut committed);
        doubled.encode(&mut committed);
        scaled.encode(&mut committed);
        committed.extend_from_slice(&counts.adds.to_le_bytes());
        committed.extend_from_slice(&counts.doubles.to_le_bytes());
        totals.adds += counts.adds;
        totals.doubles += counts.doubles;
        detail.push(json!({
            "curve": format!("{curve:?}"),
            "expected_raw_adds": counts.adds,
            "expected_raw_doubles": counts.doubles,
            "sum_point_words_le": sum.coordinates,
            "doubled_point_words_le": doubled.coordinates,
            "scalar_multiple_words_le": scaled.coordinates,
        }));
    }
    committed.extend_from_slice(&totals.adds.to_le_bytes());
    committed.extend_from_slice(&totals.doubles.to_le_bytes());
    Ok((
        committed,
        json!({
            "scalar": format!("0x{:x}", GUEST_SCALAR[0]),
            "expected_total_raw_adds": totals.adds,
            "expected_total_raw_doubles": totals.doubles,
            "per_curve": detail,
        }),
    ))
}

/// Read the 32-byte committed digest out of a shard's public values.
fn committed_digest(proof: &Proof) -> Result<[u8; 32]> {
    let last = proof.shard_proofs.last().context("proof has no shards")?;
    let pv: &PublicValues<[SP1Field; 4], [SP1Field; 3], [SP1Field; 4], SP1Field> =
        last.public_values.as_slice().borrow();
    let mut out = [0u8; 32];
    for (i, word) in pv.committed_value_digest.iter().enumerate() {
        for (j, byte) in word.iter().enumerate() {
            let value = byte.as_canonical_u32();
            ensure!(value < 256, "public-value digest limb is not a byte");
            out[4 * i + j] = value as u8;
        }
    }
    Ok(out)
}

async fn run_guest(elf_path: &Path, output: &Path) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    fs::create_dir_all(output)?;
    let started = Instant::now();
    let elf = fs::read(elf_path).context("read the compiled guest ELF")?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);

    let points: Vec<(Curve, Vec<Point>)> = vec![
        (
            Curve::Vesta,
            multiples(Curve::Vesta, GUEST_POINTS_PER_CURVE),
        ),
        (
            Curve::Pallas,
            multiples(Curve::Pallas, GUEST_POINTS_PER_CURVE),
        ),
    ];
    for (curve, list) in &points {
        let m = modulus(*curve);
        for p in list {
            ensure!(on_curve(p, &m), "guest fixture point is off curve");
        }
    }
    let input = guest_input_blob(&points);
    let (expected_bytes, expectation) = expected_commitment(&points, &input)?;
    let expected_digest: [u8; 32] = Sha256::digest(&expected_bytes).into();

    let mut stdin = SP1Stdin::new();
    stdin.write_slice(&input);

    let trace_start = Instant::now();
    let (records, cycles) =
        generate_records::<SP1Field>(program.clone(), stdin.clone(), core_opts(), [0; 4])?;
    let trace_seconds = trace_start.elapsed().as_secs_f64();

    let mut per_op = BTreeMap::new();
    for op in &OPS {
        let count: usize = records
            .iter()
            .map(|r| r.get_precompile_events(op.syscall).len())
            .sum();
        per_op.insert(op.id, count);
    }
    for op in &OPS {
        ensure!(
            per_op[op.id] > 0,
            "the compiled guest did not exercise {}; all four wrappers must run",
            op.id
        );
    }
    ensure!(
        per_op["vesta-add"] as u32 + per_op["pallas-add"] as u32
            == expectation["expected_total_raw_adds"].as_u64().unwrap() as u32,
        "raw add call count disagrees with the independent dispatch mirror"
    );
    ensure!(
        per_op["vesta-double"] as u32 + per_op["pallas-double"] as u32
            == expectation["expected_total_raw_doubles"].as_u64().unwrap() as u32,
        "raw double call count disagrees with the independent dispatch mirror"
    );

    let verifier = ShardVerifier::from_basefold_parameters(
        FriConfig::default_fri_config(),
        LOG_STACKING_HEIGHT,
        MAX_LOG_ROW_COUNT,
        Air::machine(),
    );
    let cpu = Cpu::new(verifier.clone());
    let prover = SimpleProver::new(verifier, cpu.clone());
    let setup_start = Instant::now();
    let (pk, vk) = prover.setup(program).await;
    let pk = unsafe { pk.into_inner() };
    let setup_seconds = setup_start.elapsed().as_secs_f64();
    let vk_bytes = bincode::serialize(&vk)?;
    fs::write(output.join("verification-key.bin"), &vk_bytes)?;
    let restored_vk: MachineVerifyingKey<SP1GlobalContext> = bincode::deserialize(&vk_bytes)?;

    let prove_start = Instant::now();
    let mut shards: Vec<_> = Vec::new();
    for record in &records {
        let data = cpu
            .trace_generator()
            .generate_main_traces(record.clone(), MAX_LOG_ROW_COUNT, ProverSemaphore::new(1))
            .await;
        let mut challenger = SP1GlobalContext::default_challenger();
        pk.vk.observe_into(&mut challenger);
        let (proof, permit) = cpu.prove_shard_with_data(
            ShardData {
                pk: pk.clone(),
                main_trace_data: data,
            },
            challenger,
        );
        drop(permit);
        shards.push(proof);
    }
    let generated_shards = shards.len();
    ensure!(
        generated_shards == records.len(),
        "shard assembly lost shards: {generated_shards} generated from {} execution records",
        records.len()
    );
    let proof = MachineProof {
        shard_proofs: canonical_shard_order(shards),
    };
    ensure!(
        proof.shard_proofs.len() == generated_shards,
        "canonical ordering changed the shard count"
    );
    let prove_seconds = prove_start.elapsed().as_secs_f64();

    let proof_bytes = bincode::serialize(&proof)?;
    fs::write(output.join("guest.proof.bin"), &proof_bytes)?;
    let reloaded: Proof = bincode::deserialize(&proof_bytes)?;
    let serialized_shards = reloaded.shard_proofs.len();
    // Assembly must lose nothing. This is a diagnostic, not a security check:
    // the cryptographic whole-execution verification below is the gate.
    ensure!(
        serialized_shards == generated_shards && generated_shards == records.len(),
        "shard assembly lost shards: {} records, {generated_shards} generated, {serialized_shards} serialized",
        records.len()
    );

    // The saved proof must actually contain all four Pasta chips. The previous
    // assembly dropped three of them while still reporting six shards.
    let mut chip_shards: BTreeMap<String, usize> = BTreeMap::new();
    for shard in reloaded.shard_proofs.iter() {
        for name in shard.opened_values.chips.keys() {
            *chip_shards.entry(name.clone()).or_default() += 1;
        }
    }
    let mut missing = Vec::new();
    for op in &OPS {
        if !chip_shards.contains_key(op.chip) {
            missing.push(op.chip);
        }
    }
    ensure!(
        missing.is_empty(),
        "the saved compiled-guest proof is missing Pasta chips {missing:?}; all four must appear"
    );

    let policy = Policy {
        require_successful_exit: true,
        expected_committed_digest: Some(expected_digest),
    };
    let verify_start = Instant::now();
    let complete = verify_complete_execution(&prover, &restored_vk, &reloaded, &policy);
    let verify_seconds = verify_start.elapsed().as_secs_f64();
    let accepted = complete.is_ok();
    let per_shard_accepted = prover.verify(&restored_vk, &reloaded).is_ok();
    let error = complete.as_ref().err().map(|e| e.detail.clone());
    let rejection_check = complete.as_ref().err().map(|e| e.check);

    let digest = committed_digest(&reloaded)?;
    let digest_matches = digest == expected_digest;
    fs::write(output.join("expected-committed-bytes.bin"), &expected_bytes)?;
    write_json(
        output.join("shard-inventory.json"),
        &json!({
            "execution_records": records.len(),
            "generated_shards": generated_shards,
            "serialized_shards": serialized_shards,
            "chip_shard_counts": chip_shards,
            "pasta_chips_present": OPS.iter().map(|o| o.chip).collect::<Vec<_>>(),
        }),
    )?;

    let report = json!({
        "status": if accepted && digest_matches { "passed" } else { "failed" },
        "mode": "guest",
        "elf_path": elf_path.to_string_lossy(),
        "elf_sha256": sha(&elf),
        "elf_bytes": elf.len(),
        "guest_is_compiled_elf": true,
        "inputs_from_guest_input_memory": true,
        "input_blob_sha256": sha(&input),
        "input_blob_bytes": input.len(),
        "guest_cycles": cycles,
        "execution_records": records.len(),
        "generated_shards": generated_shards,
        "serialized_shards": serialized_shards,
        "chip_shard_counts": chip_shards,
        "precompile_events": per_op,
        "expectation": expectation,
        "committed_public_value_digest_hex": hex::encode(digest),
        "independently_recomputed_digest_hex": hex::encode(expected_digest),
        "committed_digest_matches_independent_recomputation": digest_matches,
        "expected_committed_bytes": expected_bytes.len(),
        "complete_verifier_accepted": accepted,
        "per_shard_verifier_accepted": per_shard_accepted,
        "rejection_check": rejection_check,
        "verifier_error": error,
        "proof_bytes": proof_bytes.len(),
        "proof_sha256": sha(&proof_bytes),
        "verification_key_sha256": sha(&vk_bytes),
        "trace_seconds": trace_seconds,
        "setup_seconds": setup_seconds,
        "prove_seconds": prove_seconds,
        "verify_seconds": verify_seconds,
        "total_seconds": started.elapsed().as_secs_f64(),
    });
    write_json(output.join("result.json"), &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    ensure!(
        accepted,
        "the compiled guest proof was rejected by the complete verifier: {:?}",
        rejection_check
    );
    ensure!(
        digest_matches,
        "the committed public-value digest does not match the independent recomputation"
    );
    println!("GUEST_PASSED");
    Ok(())
}

/// Reload a compiled-guest core proof from disk and verify it with a key
/// derived from the pinned ELF.
///
/// `--mode guest` and `--mode guest-gpu` both verify in the process that
/// proved, under a key they had just computed themselves. That leaves the
/// property a reviewer actually wants unchecked: that the bytes on disk still
/// verify in a fresh process whose only inputs are the pinned ELF and this
/// binary's own fixture code. This mode supplies it.
///
/// Two deliberate choices. The expected committed digest is recomputed here
/// from the fixture points, never read out of the artifact directory, so a
/// tampered report cannot define its own expectation. And a saved
/// verification key is *compared* against the ELF-derived one rather than
/// used, because trusting an adjacent key only proves the bundle is
/// self-consistent.
async fn run_guest_verify(elf_path: &Path, input: &Path, output: &Path) -> Result<()> {
    ensure!(
        input.is_dir(),
        "--input must be a guest proof directory, not a single file"
    );
    ensure!(!output.exists(), "use a fresh output path");

    // Either producer's filename, but exactly one, so a directory holding both
    // cannot have one verified while the other is silently ignored.
    let cpu_proof = input.join("guest.proof.bin");
    let gpu_proof = input.join("guest-gpu.proof.bin");
    let (proof_path, proved_on) = match (cpu_proof.is_file(), gpu_proof.is_file()) {
        (true, false) => (cpu_proof, "cpu"),
        (false, true) => (gpu_proof, "cuda"),
        (true, true) => bail!(
            "{} holds both guest.proof.bin and guest-gpu.proof.bin; verify one directory at a time",
            input.display()
        ),
        (false, false) => bail!(
            "{} holds neither guest.proof.bin nor guest-gpu.proof.bin",
            input.display()
        ),
    };

    // The PCS parameters must match whichever producer wrote the proof: the
    // CPU `guest` mode uses the default FRI config at 17/17, while the CUDA
    // path defaults to the production core config at 21/22. Same env names as
    // the prover side, so one exported set covers both.
    let fri_name = std::env::var("PASTA_FRI_CONFIG").unwrap_or_else(|_| "default".to_string());
    let fri = match fri_name.as_str() {
        "core" => sp1_primitives::fri_params::core_fri_config(),
        "default" => FriConfig::default_fri_config(),
        other => bail!("unknown PASTA_FRI_CONFIG {other}; use core or default"),
    };
    let log_stacking: u32 = match std::env::var("PASTA_LOG_STACKING_HEIGHT") {
        Ok(v) => v
            .parse()
            .with_context(|| format!("PASTA_LOG_STACKING_HEIGHT={v} is not a number"))?,
        Err(_) => LOG_STACKING_HEIGHT,
    };
    let max_rows: usize = match std::env::var("PASTA_MAX_LOG_ROW_COUNT") {
        Ok(v) => v
            .parse()
            .with_context(|| format!("PASTA_MAX_LOG_ROW_COUNT={v} is not a number"))?,
        Err(_) => MAX_LOG_ROW_COUNT,
    };

    let verifier =
        ShardVerifier::from_basefold_parameters(fri, log_stacking, max_rows, Air::machine());
    let cpu = Cpu::new(verifier.clone());
    let prover = SimpleProver::new(verifier, cpu);

    // Program identity comes from the pinned ELF and nothing else.
    let elf = fs::read(elf_path).context("read the compiled guest ELF")?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);
    let setup_start = Instant::now();
    let (_pk, derived_vk) = prover.setup(program).await;
    let setup_seconds = setup_start.elapsed().as_secs_f64();
    let derived_bytes = bincode::serialize(&derived_vk)?;

    let saved_key_path = input.join("verification-key.bin");
    let saved_key = if saved_key_path.is_file() {
        let bytes = fs::read(&saved_key_path).context("read the saved verification key")?;
        ensure!(
            bytes == derived_bytes,
            "the saved verification key does not match one derived from the pinned ELF at {}",
            elf_path.display()
        );
        json!({
            "path": saved_key_path.to_string_lossy(),
            "sha256": sha(&bytes),
            "matches_elf_derived_key": true,
        })
    } else {
        json!({
            "present": false,
            "note": "no saved key in the directory; verification used the ELF-derived key",
        })
    };

    // Recompute the expectation from the fixture, exactly as `--mode guest`
    // builds it, so the digest this gate requires is independent of the run.
    let points: Vec<(Curve, Vec<Point>)> = vec![
        (
            Curve::Vesta,
            multiples(Curve::Vesta, GUEST_POINTS_PER_CURVE),
        ),
        (
            Curve::Pallas,
            multiples(Curve::Pallas, GUEST_POINTS_PER_CURVE),
        ),
    ];
    for (curve, list) in &points {
        let m = modulus(*curve);
        for p in list {
            ensure!(on_curve(p, &m), "guest fixture point is off curve");
        }
    }
    let input_blob = guest_input_blob(&points);
    let (expected_bytes, expectation) = expected_commitment(&points, &input_blob)?;
    let expected_digest: [u8; 32] = Sha256::digest(&expected_bytes).into();

    let proof_bytes = fs::read(&proof_path).context("read the saved guest proof")?;
    let reloaded: Proof = bincode::deserialize(&proof_bytes)
        .context("deserialize the saved guest proof")?;
    let shard_count = reloaded.shard_proofs.len();
    ensure!(shard_count > 0, "the saved proof has no shards");

    let mut chip_shards: BTreeMap<String, usize> = BTreeMap::new();
    for shard in reloaded.shard_proofs.iter() {
        for name in shard.opened_values.chips.keys() {
            *chip_shards.entry(name.clone()).or_default() += 1;
        }
    }
    let missing: Vec<_> = OPS
        .iter()
        .filter(|op| !chip_shards.contains_key(op.chip))
        .map(|op| op.chip)
        .collect();
    ensure!(
        missing.is_empty(),
        "the reloaded proof is missing Pasta chips {missing:?}; all four must appear"
    );

    // If the producer recorded an inventory, the reload must agree with it.
    let inventory_path = input.join("shard-inventory.json");
    let inventory = if inventory_path.is_file() {
        let v: Value = serde_json::from_slice(&fs::read(&inventory_path)?)
            .context("parse shard-inventory.json")?;
        if let Some(saved) = v["serialized_shards"].as_u64() {
            ensure!(
                saved as usize == shard_count,
                "shard-inventory.json records {saved} serialized shards; the reloaded proof has \
                 {shard_count}"
            );
        }
        v
    } else {
        json!(null)
    };

    let policy = Policy {
        require_successful_exit: true,
        expected_committed_digest: Some(expected_digest),
    };
    let verify_start = Instant::now();
    let complete = verify_complete_execution(&prover, &derived_vk, &reloaded, &policy);
    let verify_seconds = verify_start.elapsed().as_secs_f64();
    let accepted = complete.is_ok();
    let rejection_check = complete.as_ref().err().map(|e| e.check);
    let error = complete.as_ref().err().map(|e| e.detail.clone());

    let digest = committed_digest(&reloaded)?;
    let digest_matches = digest == expected_digest;

    // The gate must not be vacuous. An empty proof and a changed expected
    // output both have to be rejected, and the rejecting layer is recorded as
    // the verifier names it rather than asserted to be one in particular.
    let empty = Proof {
        shard_proofs: Vec::new(),
    };
    let empty_result = verify_complete_execution(&prover, &derived_vk, &empty, &policy);
    let empty_rejected = empty_result.is_err();
    let empty_check = empty_result.as_ref().err().map(|e| e.check);

    let mut altered = expected_digest;
    altered[0] ^= 1;
    let altered_policy = Policy {
        require_successful_exit: true,
        expected_committed_digest: Some(altered),
    };
    let altered_result = verify_complete_execution(&prover, &derived_vk, &reloaded, &altered_policy);
    let altered_rejected = altered_result.is_err();
    let altered_check = altered_result.as_ref().err().map(|e| e.check);

    let passed =
        accepted && digest_matches && empty_rejected && altered_rejected;

    let report = json!({
        "status": if passed { "passed" } else { "failed" },
        "mode": "guest-verify",
        "proved_on": proved_on,
        "verified_on": "cpu",
        "fresh_process_reload": true,
        "proof_generated": false,
        "elf_path": elf_path.to_string_lossy(),
        "elf_sha256": sha(&elf),
        "elf_bytes": elf.len(),
        "proof_path": proof_path.to_string_lossy(),
        "proof_bytes": proof_bytes.len(),
        "proof_sha256": sha(&proof_bytes),
        "fri_config": fri_name,
        "log_stacking_height": log_stacking,
        "max_log_row_count": max_rows,
        "verification_key_derived_from_elf_sha256": sha(&derived_bytes),
        "saved_verification_key": saved_key,
        "serialized_shards": shard_count,
        "chip_shard_counts": chip_shards,
        "pasta_chips_present": OPS.iter().map(|o| o.chip).collect::<Vec<_>>(),
        "expectation": expectation,
        "independently_recomputed_digest_hex": hex::encode(expected_digest),
        "committed_public_value_digest_hex": hex::encode(digest),
        "committed_digest_matches_independent_recomputation": digest_matches,
        "complete_verifier_accepted": accepted,
        "rejection_check": rejection_check,
        "verifier_error": error,
        "empty_proof_rejected": empty_rejected,
        "empty_proof_rejecting_check": empty_check,
        "altered_expected_digest_rejected": altered_rejected,
        "altered_expected_digest_rejecting_check": altered_check,
        "shard_inventory": inventory,
        "setup_seconds": setup_seconds,
        "verify_seconds": verify_seconds,
    });
    write_json(output, &report)?;

    ensure!(
        accepted,
        "the reloaded guest proof was rejected by the complete verifier: {rejection_check:?}"
    );
    ensure!(
        digest_matches,
        "the committed public-value digest does not match the independent recomputation"
    );
    ensure!(
        empty_rejected,
        "the complete verifier accepted an empty proof; the gate is vacuous"
    );
    ensure!(
        altered_rejected,
        "the complete verifier accepted a changed expected public output"
    );
    println!("GUEST_VERIFY_PASSED");
    Ok(())
}

/// Malformed-input tests for the compiled guest: the guest's own proved checks
/// must stop execution, so no successful proof of a bad input exists.
fn run_guest_malformed(elf_path: &Path, output: &Path) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output path");
    let elf = fs::read(elf_path)?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);
    let silence = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut results = Vec::new();
    let base: Vec<(Curve, Vec<Point>)> = vec![
        (
            Curve::Vesta,
            multiples(Curve::Vesta, GUEST_POINTS_PER_CURVE),
        ),
        (
            Curve::Pallas,
            multiples(Curve::Pallas, GUEST_POINTS_PER_CURVE),
        ),
    ];
    let vesta_m = modulus(Curve::Vesta);
    let mut cases: Vec<(&str, Vec<u8>, &str)> = Vec::new();

    cases.push((
        "honest-control",
        guest_input_blob(&base),
        "the valid blob must execute to a successful halt",
    ));

    let mut off_curve = base.clone();
    off_curve[0].1[1].1 = (&off_curve[0].1[1].1 + BigUint::one()) % &vesta_m;
    cases.push((
        "vesta-off-curve-point",
        guest_input_blob(&off_curve),
        "on-curve validation must reject y^2 != x^3 + 5",
    ));

    let mut noncanonical = base.clone();
    noncanonical[0].1[1].0 = &noncanonical[0].1[1].0 + &vesta_m;
    cases.push((
        "vesta-noncanonical-x",
        guest_input_blob(&noncanonical),
        "canonicality validation must reject x >= modulus",
    ));

    let mut pallas_off = base.clone();
    let pallas_m = modulus(Curve::Pallas);
    pallas_off[1].1[2].1 = (&pallas_off[1].1[2].1 + BigUint::one()) % &pallas_m;
    cases.push((
        "pallas-off-curve-point",
        guest_input_blob(&pallas_off),
        "on-curve validation must reject an off-curve Pallas point",
    ));

    let mut cross_curve = base.clone();
    cross_curve[1].1[0] = base[0].1[0].clone();
    cases.push(("pallas-slot-holds-vesta-point", guest_input_blob(&cross_curve), "a Vesta point is generally off the curve over Pallas's base field, so validation must reject it"));

    let mut truncated = guest_input_blob(&base);
    truncated.truncate(truncated.len() - 8);
    cases.push((
        "truncated-blob",
        truncated,
        "the header length check must reject a short blob",
    ));

    let mut bad_magic = guest_input_blob(&base);
    bad_magic[0] = b'X';
    cases.push((
        "wrong-magic",
        bad_magic,
        "the magic check must reject an unknown input format",
    ));

    let mut identity_input = base.clone();
    identity_input[0].1[1] = (BigUint::zero(), BigUint::zero());
    cases.push((
        "vesta-identity-encoding",
        guest_input_blob(&identity_input),
        "the all-zero encoding is accepted only as the identity and never forwarded to a syscall",
    ));

    for (id, blob, intent) in cases {
        let mut stdin = SP1Stdin::new();
        stdin.write_slice(&blob);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            generate_records::<SP1Field>(program.clone(), stdin.clone(), core_opts(), [0; 4])
        }));
        let (status, detail) = match outcome {
            Ok(Ok((records, cycles))) => {
                let events: usize = records
                    .iter()
                    .map(|r| {
                        OPS.iter()
                            .map(|o| r.get_precompile_events(o.syscall).len())
                            .sum::<usize>()
                    })
                    .sum();
                // A failed guest assertion still halts, so `generate_records`
                // returns Ok. What distinguishes a rejection is the exit code,
                // which the proof binds in its public values: a nonzero exit
                // code means there is no successful execution to prove.
                let exit_code = records
                    .last()
                    .map(|r| r.public_values.exit_code)
                    .unwrap_or_default();
                let detail = json!({
                    "cycles": cycles,
                    "precompile_events": events,
                    "exit_code": exit_code,
                });
                if exit_code == 0 {
                    ("executed_successfully".to_string(), detail)
                } else {
                    ("guest_rejected_with_nonzero_exit_code".to_string(), detail)
                }
            }
            Ok(Err(error)) => (
                "guest_execution_failed".to_string(),
                json!(error.to_string()),
            ),
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_else(|| "non-string panic payload".into());
                ("guest_execution_panicked".to_string(), json!(message))
            }
        };
        let expected_success = id == "honest-control" || id == "vesta-identity-encoding";
        let observed_calls = detail.get("precompile_events").and_then(Value::as_u64);
        let observed_exit = detail.get("exit_code").and_then(Value::as_u64);
        // A rejection must be *observed*, not inferred: the guest must have run
        // to a recorded nonzero exit code with a known zero Pasta call count. A
        // host panic or execution error is a harness failure, not a pass.
        ensure!(
            status != "guest_execution_panicked",
            "{id}: execution panicked on the host; that is a harness failure, not a guest rejection"
        );
        ensure!(
            status != "guest_execution_failed",
            "{id}: the executor returned an error; that is a harness failure, not a guest rejection"
        );
        ensure!(
            observed_calls.is_some() && observed_exit.is_some(),
            "{id}: missing exit-code or precompile-count observation"
        );
        let passed = if expected_success {
            status == "executed_successfully"
                && observed_exit == Some(0)
                && observed_calls.map(|n| n > 0).unwrap_or(false)
        } else {
            status == "guest_rejected_with_nonzero_exit_code"
                && observed_exit.map(|c| c != 0).unwrap_or(false)
                && observed_calls == Some(0)
        };
        println!("GUEST_MALFORMED {id} {status} passed={passed}");
        results.push(json!({
            "case": id,
            "intent": intent,
            "input_blob_sha256": sha(&blob),
            "outcome": status,
            "detail": detail,
            "expected_successful_execution": expected_success,
            "passed": passed,
            "observed_exit_code": observed_exit,
            "observed_pasta_calls": observed_calls,
            "stopped_before_any_raw_precompile_call": observed_calls == Some(0),
            "rejection_level": if status == "executed_successfully" { Value::Null } else { json!("proved guest code: the program halts with a nonzero exit code, which the proof binds in its public values, so no proof of a successful run on this input exists") },
        }));
    }
    std::panic::set_hook(silence);
    let failed: Vec<&Value> = results
        .iter()
        .filter(|r| r["passed"] != json!(true))
        .collect();
    let rejected = results
        .iter()
        .filter(|r| r["expected_successful_execution"] == json!(false))
        .count();
    let controls = results.len() - rejected;
    let summary = json!({
        "status": if failed.is_empty() { "passed" } else { "failed" },
        "mode": "guest-malformed",
        "elf_sha256": sha(&elf),
        "test_kind": "execution tests: the guest's own proved checks stop the program, so no successful execution of a bad input exists to prove",
        "not_cryptographic_tamper_tests": true,
        "total_cases": results.len(),
        "rejected_malformed_inputs": rejected,
        "successful_controls": controls,
        "note": "these are guest-level execution rejections, distinct from the per-case cryptographic tamper matrix. Each rejection is evidenced by a recorded nonzero exit code and zero Pasta precompile calls.",
        "cases": results,
    });
    write_json(output, &summary)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    ensure!(
        failed.is_empty(),
        "compiled-guest malformed-input tests failed"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Mode: volume (per-chip trace volume and small-batch proving cost)
// ---------------------------------------------------------------------------

/// Data pages for volume sweeps sit above a longer instruction region.
const VOLUME_BASE: u64 = 0x40_0000;

/// Build `count` calls of one operation, or a round-robin mix of all four.
fn volume_calls(spec: &str, count: usize, reuse: bool) -> Result<Vec<Call>> {
    // A single reused memory slot cannot serve both base fields: the second
    // curve would reinterpret the first curve's point over its own field,
    // which is not a curve-valid input. The earlier reuse-mixed fixture did
    // exactly that and its numbers were invalid.
    ensure!(
        !(reuse && spec == "mixed"),
        "reused slots cannot be shared across curves; run one operation at a time, \
         or drop --reuse-slots for a mixed sweep"
    );
    const POOL: usize = 16;
    let mut pools: BTreeMap<&str, Vec<Point>> = BTreeMap::new();
    for curve in [Curve::Vesta, Curve::Pallas] {
        let key = if curve == Curve::Vesta {
            "vesta"
        } else {
            "pallas"
        };
        pools.insert(key, multiples(curve, POOL + 1));
    }
    let chosen: Vec<Op> = if spec == "mixed" {
        OPS.to_vec()
    } else {
        vec![*OPS
            .iter()
            .find(|o| o.id == spec)
            .with_context(|| format!("unknown operation {spec}"))?]
    };
    let mut calls = Vec::with_capacity(count);
    for i in 0..count {
        let op = chosen[i % chosen.len()];
        let key = if op.curve == Curve::Vesta {
            "vesta"
        } else {
            "pallas"
        };
        let pool = &pools[key];
        // Walk the pool so successive rows carry different coordinates.
        let a = i % POOL;
        let b = (a + 1) % POOL;
        let p = pool[a].clone();
        let q = if a == b {
            pool[b + 1].clone()
        } else {
            pool[b].clone()
        };
        calls.push(match op.kind {
            Kind::Add => make_call(op, p, Some(q))?,
            Kind::Double => make_call(op, p, None)?,
        });
    }
    Ok(calls)
}

/// Per-chip committed trace volume, summed over every shard of one execution.
#[derive(Default, Clone)]
struct ChipVolume {
    real_rows: u64,
    padded_rows: u64,
    cells: u64,
    width: usize,
    shards_present: u32,
}

async fn measure_volume(
    cpu: &Cpu,
    records: &[ExecutionRecord],
) -> Result<BTreeMap<String, ChipVolume>> {
    let mut totals: BTreeMap<String, ChipVolume> = BTreeMap::new();
    for record in records {
        let data = cpu
            .trace_generator()
            .generate_main_traces(record.clone(), MAX_LOG_ROW_COUNT, ProverSemaphore::new(1))
            .await;
        for (name, trace) in data.traces.iter() {
            let width = trace.num_polynomials();
            let real = trace.num_real_entries() as u64;
            let padded = 1u64 << trace.num_variables();
            let entry = totals.entry(name.clone()).or_default();
            entry.width = width;
            entry.real_rows += real;
            entry.padded_rows += padded;
            entry.cells += real * width as u64;
            if real > 0 {
                entry.shards_present += 1;
            }
        }
    }
    Ok(totals)
}

fn volume_json(totals: &BTreeMap<String, ChipVolume>) -> Value {
    let mut chips = Vec::new();
    let mut total_cells = 0u64;
    let mut precompile_cells = 0u64;
    for (name, v) in totals {
        if v.real_rows == 0 {
            continue;
        }
        total_cells += v.cells;
        if OPS.iter().any(|o| o.chip == *name) {
            precompile_cells += v.cells;
        }
        chips.push(json!({
            "chip": name,
            "width": v.width,
            "real_rows": v.real_rows,
            "padded_rows": v.padded_rows,
            "cells": v.cells,
            "shards_present": v.shards_present,
        }));
    }
    chips.sort_by_key(|c| std::cmp::Reverse(c["cells"].as_u64().unwrap_or(0)));
    json!({
        "chips": chips,
        "total_committed_cells": total_cells,
        "precompile_cells": precompile_cells,
        "core_and_fixed_cells": total_cells - precompile_cells,
    })
}

async fn run_volume(
    output: &Path,
    spec: &str,
    counts: &[usize],
    prove_counts: &[usize],
    compare: bool,
    reuse: bool,
    pad_alu: usize,
) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output directory");
    fs::create_dir_all(output)?;
    let started = Instant::now();
    let mut rows = Vec::new();

    for &count in counts {
        let calls = volume_calls(spec, count, reuse)?;
        let (program, fixture) =
            build_program_at(&calls, VOLUME_BASE, compare, reuse, pad_alu, Some(0), 0, 0)?;
        let instructions = fixture["instructions"].as_u64().unwrap();
        let trace_start = Instant::now();
        let (records, cycles) =
            generate_records::<SP1Field>(program.clone(), SP1Stdin::new(), core_opts(), [0; 4])?;
        let trace_seconds = trace_start.elapsed().as_secs_f64();

        let mut events: BTreeMap<&str, usize> = BTreeMap::new();
        for op in &OPS {
            let n: usize = records
                .iter()
                .map(|r| r.get_precompile_events(op.syscall).len())
                .sum();
            if n > 0 {
                events.insert(op.id, n);
            }
        }
        let event_total: usize = events.values().sum();
        ensure!(
            event_total == count,
            "expected {count} precompile events, executor produced {event_total}"
        );

        let verifier = ShardVerifier::from_basefold_parameters(
            FriConfig::default_fri_config(),
            LOG_STACKING_HEIGHT,
            MAX_LOG_ROW_COUNT,
            Air::machine(),
        );
        let cpu = Cpu::new(verifier.clone());
        let totals = measure_volume(&cpu, &records).await?;
        let volume = volume_json(&totals);

        let mut row = json!({
            "operation_spec": spec,
            "calls": count,
            "output_comparisons_emitted": compare,
            "memory_slots_reused": reuse,
            "alu_filler_per_call": pad_alu,
            "instructions": instructions,
            "guest_cycles": cycles,
            "shards": records.len(),
            "precompile_events": events,
            "trace_generation_seconds": trace_seconds,
            "volume": volume,
            "proved": false,
        });

        if prove_counts.contains(&count) {
            let prover = SimpleProver::new(verifier, cpu.clone());
            let (pk, vk) = prover.setup(program).await;
            let pk = unsafe { pk.into_inner() };
            let vk_bytes = bincode::serialize(&vk)?;
            let restored: MachineVerifyingKey<SP1GlobalContext> = bincode::deserialize(&vk_bytes)?;
            let prove_start = Instant::now();
            let mut shards: Vec<_> = Vec::new();
            for record in &records {
                let data = cpu
                    .trace_generator()
                    .generate_main_traces(
                        record.clone(),
                        MAX_LOG_ROW_COUNT,
                        ProverSemaphore::new(1),
                    )
                    .await;
                let mut challenger = SP1GlobalContext::default_challenger();
                pk.vk.observe_into(&mut challenger);
                let (proof, permit) = cpu.prove_shard_with_data(
                    ShardData {
                        pk: pk.clone(),
                        main_trace_data: data,
                    },
                    challenger,
                );
                drop(permit);
                shards.push(proof);
            }
            ensure!(
                shards.len() == records.len(),
                "shard assembly lost shards in the volume path: {} of {}",
                shards.len(),
                records.len()
            );
            let proof = MachineProof {
                shard_proofs: canonical_shard_order(shards),
            };
            let prove_seconds = prove_start.elapsed().as_secs_f64();
            let proof_bytes = bincode::serialize(&proof)?;
            let verify_start = Instant::now();
            // Gate on the complete verifier, never on the per-shard endpoint:
            // that one accepts an empty proof and cannot see a dropped,
            // duplicated or reordered shard.
            let policy = Policy {
                require_successful_exit: true,
                expected_committed_digest: Some(fixture_digest_bytes(0)),
            };
            let verification = verify_complete_execution(&prover, &restored, &proof, &policy);
            let verify_seconds = verify_start.elapsed().as_secs_f64();
            let accepted = verification.is_ok();
            ensure!(
                accepted,
                "volume sweep proof was rejected by the complete verifier: {:?}",
                verification.as_ref().err().map(|e| e.check)
            );
            let object = row.as_object_mut().unwrap();
            object.insert("proved".into(), json!(true));
            object.insert("prove_seconds".into(), json!(prove_seconds));
            object.insert("verify_seconds".into(), json!(verify_seconds));
            object.insert("proof_bytes".into(), json!(proof_bytes.len()));
            object.insert("complete_verifier_accepted".into(), json!(accepted));
        }

        println!(
            "VOLUME {spec} calls={count} cycles={cycles} shards={} precompile_cells={} total_cells={} proved={}",
            records.len(),
            row["volume"]["precompile_cells"],
            row["volume"]["total_committed_cells"],
            row["proved"],
        );
        write_json(output.join(format!("{spec}-{count:06}.json")), &row)?;
        rows.push(row);
    }

    let summary = json!({
        "status": "passed",
        "mode": "volume",
        "operation_spec": spec,
        "output_comparisons_emitted": compare,
        "memory_slots_reused": reuse,
        "alu_filler_per_call": pad_alu,
        "note": "committed cells are real_rows * width summed over shards; padded_rows is the MLE capacity. Proving figures are single local runs of small programs in which the fixed Byte/Range tables dominate.",
        "log_stacking_height": LOG_STACKING_HEIGHT,
        "max_log_row_count": MAX_LOG_ROW_COUNT,
        "core_opts": core_opts(),
        "rows": rows,
        "total_seconds": started.elapsed().as_secs_f64(),
    });
    write_json(output.join("result.json"), &summary)?;
    println!("VOLUME_PASSED {spec}");
    Ok(())
}

/// Measure per-chip trace volume for a compiled guest ELF, without proving.
/// This is the independent cross-check on the synthetic sweeps' per-call model.
async fn run_elf_volume(elf_path: &Path, output: &Path) -> Result<()> {
    ensure!(!output.exists(), "use a fresh output path");
    let elf = fs::read(elf_path).context("read the compiled guest ELF")?;
    let program = Arc::new(Program::from(&elf).map_err(|e| anyhow::anyhow!("{e}"))?);
    let points: Vec<(Curve, Vec<Point>)> = vec![
        (
            Curve::Vesta,
            multiples(Curve::Vesta, GUEST_POINTS_PER_CURVE),
        ),
        (
            Curve::Pallas,
            multiples(Curve::Pallas, GUEST_POINTS_PER_CURVE),
        ),
    ];
    let input = guest_input_blob(&points);
    let mut stdin = SP1Stdin::new();
    stdin.write_slice(&input);
    let (records, cycles) = generate_records::<SP1Field>(program, stdin, core_opts(), [0; 4])?;
    let verifier = ShardVerifier::from_basefold_parameters(
        FriConfig::default_fri_config(),
        LOG_STACKING_HEIGHT,
        MAX_LOG_ROW_COUNT,
        Air::machine(),
    );
    let cpu = Cpu::new(verifier);
    let totals = measure_volume(&cpu, &records).await?;
    let volume = volume_json(&totals);
    let mut events = BTreeMap::new();
    for op in &OPS {
        let n: usize = records
            .iter()
            .map(|r| r.get_precompile_events(op.syscall).len())
            .sum();
        events.insert(op.id, n);
    }
    let add_calls = events["vesta-add"] + events["pallas-add"];
    let double_calls = events["vesta-double"] + events["pallas-double"];
    let report = json!({
        "status": "passed",
        "mode": "elf-volume",
        "elf_sha256": sha(&elf),
        "guest_cycles": cycles,
        "shards": records.len(),
        "precompile_events": events,
        "raw_add_calls": add_calls,
        "raw_double_calls": double_calls,
        "volume": volume,
        "note": "a compiled-guest data point for the per-call model derived from the synthetic sweeps; the guest's own instruction mix differs from the sweep fixtures",
    });
    write_json(output, &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("ELF_VOLUME_PASSED");
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut mode = "registration".to_string();
    let mut output: Option<PathBuf> = None;
    let mut input: Option<PathBuf> = None;
    let mut elf: Option<PathBuf> = None;
    let mut reference: Option<PathBuf> = None;
    let mut manifest: Option<PathBuf> = None;
    let mut arm: Option<String> = None;
    let mut time_witness = false;
    let mut max_records: Option<usize> = None;
    let mut spec = "mixed".to_string();
    let mut counts: Vec<usize> = vec![1, 2, 4, 8, 16, 32, 64, 128, 256];
    let mut prove_counts: Vec<usize> = Vec::new();
    let mut compare = true;
    let mut reuse = false;
    let mut pad_alu: usize = 0;
    let mut ops: Vec<Op> = OPS.to_vec();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                mode = args.get(i + 1).context("--mode needs a value")?.clone();
                i += 2;
            }
            "--output" => {
                output = Some(PathBuf::from(
                    args.get(i + 1).context("--output needs a value")?,
                ));
                i += 2;
            }
            "--arm" => {
                arm = Some(args.get(i + 1).context("--arm needs a value")?.clone());
                i += 2;
            }
            "--manifest" => {
                manifest = Some(PathBuf::from(
                    args.get(i + 1).context("--manifest needs a value")?,
                ));
                i += 2;
            }
            "--spec" => {
                spec = args.get(i + 1).context("--spec needs a value")?.clone();
                i += 2;
            }
            "--calls" => {
                counts = args
                    .get(i + 1)
                    .context("--calls needs a value")?
                    .split(',')
                    .map(|v| v.parse::<usize>().context("bad --calls entry"))
                    .collect::<Result<Vec<_>>>()?;
                i += 2;
            }
            "--prove-calls" => {
                prove_counts = args
                    .get(i + 1)
                    .context("--prove-calls needs a value")?
                    .split(',')
                    .filter(|v| !v.is_empty())
                    .map(|v| v.parse::<usize>().context("bad --prove-calls entry"))
                    .collect::<Result<Vec<_>>>()?;
                i += 2;
            }
            "--reference" => {
                reference = Some(PathBuf::from(
                    args.get(i + 1).context("--reference needs a value")?,
                ));
                i += 2;
            }
            "--time-witness" => {
                time_witness = true;
                i += 1;
            }
            "--max-records" => {
                max_records = Some(
                    args.get(i + 1)
                        .context("--max-records needs a value")?
                        .parse()?,
                );
                i += 2;
            }
            "--no-compare" => {
                compare = false;
                i += 1;
            }
            "--pad-alu" => {
                pad_alu = args
                    .get(i + 1)
                    .context("--pad-alu needs a value")?
                    .parse()
                    .context("bad --pad-alu")?;
                i += 2;
            }
            "--reuse-slots" => {
                reuse = true;
                compare = false;
                i += 1;
            }
            "--elf" => {
                elf = Some(PathBuf::from(
                    args.get(i + 1).context("--elf needs a value")?,
                ));
                i += 2;
            }
            "--input" => {
                input = Some(PathBuf::from(
                    args.get(i + 1).context("--input needs a value")?,
                ));
                i += 2;
            }
            "--ops" => {
                let list = args.get(i + 1).context("--ops needs a value")?;
                ops = list
                    .split(',')
                    .map(|name| {
                        OPS.iter()
                            .find(|op| op.id == name)
                            .copied()
                            .with_context(|| format!("unknown operation {name}"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                i += 2;
            }
            other => bail!("unexpected argument {other}"),
        }
    }
    let generates_proof = matches!(
        mode.as_str(),
        "prove"
            | "continuity"
            | "structural"
            | "guest"
            | "guest-gpu"
            | "r5-gpu"
            | "manifest-core-gpu"
            | "pipeline-cpu"
            | "pasta-fp-check"
    ) || (mode == "volume" && !prove_counts.is_empty());
    ensure!(!generates_proof || std::env::var("PASTA_ALLOW_REAL_PROOF").as_deref() == Ok("1"),
        "real proof generation is gated: authorize it on a suitably sized host, then set PASTA_ALLOW_REAL_PROOF=1");

    // The CUDA prover uses `spawn_blocking` and a task pool, so it needs a
    // multi-threaded runtime. Every CPU mode keeps the single-threaded one it
    // was measured on.
    let runtime = if mode == "guest-gpu" {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
    };
    match mode.as_str() {
        "prove" => {
            let output = output.context("--output is required")?;
            runtime.block_on(run_prove(&output, &ops))
        }
        "manifest-core-verify" => {
            let spec = manifest.context("--manifest is required")?;
            let elf = elf.context("--elf is required")?;
            let input = input.context("--input is required (downloaded results directory)")?;
            let expected =
                reference.context("--reference is required (expected public-values.bin)")?;
            let output = output.context("--output is required")?;
            runtime.block_on(run_manifest_verify(&spec, &elf, &input, &expected, &output))
        }
        "verify" => {
            let input = input.context("--input is required")?;
            let output = output.context("--output is required")?;
            runtime.block_on(run_verify(&input, &output))
        }
        "elf-volume" => {
            let elf = elf.context("--elf is required")?;
            let output = output.context("--output is required")?;
            runtime.block_on(run_elf_volume(&elf, &output))
        }
        "vk" => {
            let elf = elf.context("--elf is required")?;
            runtime.block_on(run_vk(&elf))
        }
        "continuity" => {
            let output = output.context("--output is required")?;
            runtime.block_on(run_continuity(&output))
        }
        "structural" => {
            let output = output.context("--output is required")?;
            runtime.block_on(run_structural(&output))
        }
        "inspect-proof" => {
            let input = input.context("--input is required")?;
            let report = inspect_proof(&input)?;
            if let Some(path) = output {
                write_json(path, &report)?;
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        "volume" => {
            let output = output.context("--output is required")?;
            runtime.block_on(run_volume(
                &output,
                &spec,
                &counts,
                &prove_counts,
                compare,
                reuse,
                pad_alu,
            ))
        }
        "guest" => {
            let elf = elf.context("--elf is required")?;
            let output = output.context("--output is required")?;
            runtime.block_on(run_guest(&elf, &output))
        }
        "r5-gpu" => {
            let elf = elf.context("--elf is required")?;
            let output = output.context("--output is required")?;
            let dir = input.context("--input is required (the proof-input directory)")?;
            #[cfg(feature = "cuda")]
            {
                runtime.block_on(gpu::run_r5_gpu(&elf, &dir, &output))
            }
            #[cfg(not(feature = "cuda"))]
            {
                let _ = (&elf, &output, &dir);
                bail!("r5-gpu needs the `cuda` feature; rebuild with --features cuda")
            }
        }
        // One arm of a manifest-driven core-proof run. The manifest carries
        // the program identity, the expected outcome and every SP1CoreOpts
        // field for the arm; `--arm` selects which.
        "manifest-core-gpu" => {
            let spec = manifest.context("--manifest is required")?;
            let elf = elf.context("--elf is required")?;
            let dir = input.context("--input is required (the proof-input directory)")?;
            let output = output.context("--output is required")?;
            let arm = arm.context("--arm is required (an arm name from the manifest)")?;
            #[cfg(feature = "cuda")]
            {
                runtime.block_on(gpu::run_manifest_gpu(&spec, &arm, &elf, &dir, &output))
            }
            #[cfg(not(feature = "cuda"))]
            {
                // Still validate everything that does not need a GPU, so a
                // typo in the manifest or arm name surfaces before renting.
                let _ = &output;
                let report =
                    pasta_precompile_proof::run_manifest::preflight(&spec, Some(&elf), &dir)?;
                let names: Vec<&str> = report["arms"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x["name"].as_str()).collect())
                    .unwrap_or_default();
                ensure!(
                    names.contains(&arm.as_str()),
                    "no arm named {arm:?}; manifest has {names:?}"
                );
                bail!(
                    "manifest-core-gpu needs the `cuda` feature; rebuild with --features cuda. \
                     The manifest, ELF, inputs and arm name all validated."
                )
            }
        }
        "guest-gpu" => {
            let elf = elf.context("--elf is required")?;
            let output = output.context("--output is required")?;
            #[cfg(feature = "cuda")]
            {
                runtime.block_on(gpu::run_guest_gpu(&elf, &output, reference.as_deref()))
            }
            #[cfg(not(feature = "cuda"))]
            {
                let _ = (&elf, &output, &reference);
                bail!("guest-gpu needs the `cuda` feature; rebuild with --features cuda")
            }
        }
        // Fresh-process reload of a guest proof, verified under a key derived
        // from the pinned ELF. Generates nothing, so it needs no proof gate.
        "guest-verify" => {
            let elf = elf.context("--elf is required")?;
            let input = input.context("--input is required (a guest proof directory)")?;
            let output = output.context("--output is required")?;
            runtime.block_on(run_guest_verify(&elf, &input, &output))
        }
        "guest-malformed" => {
            let elf = elf.context("--elf is required")?;
            let output = output.context("--output is required")?;
            run_guest_malformed(&elf, &output)
        }
        "analysis" => {
            let output = output.context("--output is required")?;
            run_analysis(&output)
        }
        "r5-cycles" => {
            let elf = elf.context("--elf is required")?;
            let dir = input.context("--input is required (the proof-input directory)")?;
            let output = output.context("--output is required")?;
            runtime.block_on(run_r5_cycles(
                &elf,
                &dir,
                &output,
                time_witness,
                max_records,
            ))
        }
        // Every identity the run manifest pins, checked without proving.
        // Run it before renting and again on the rented host: a hash mismatch
        // or a malformed manifest then costs nothing instead of surfacing
        // part-way through a paid run.
        "manifest-check" => {
            let spec = manifest.context("--manifest is required")?;
            let dir = input.context("--input is required (the proof-input directory)")?;
            let report =
                pasta_precompile_proof::run_manifest::preflight(&spec, elf.as_deref(), &dir)?;
            if let Some(path) = output {
                write_json(path, &report)?;
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        "trace-accounting" => {
            let elf = elf.context("--elf is required")?;
            let dir = input.context("--input is required (the proof-input directory)")?;
            let output = output.context("--output is required")?;
            runtime.block_on(run_trace_accounting(
                &elf,
                &dir,
                &output,
                max_records,
                &reference.context("--reference trace expectation is required")?,
            ))
        }
        "producer-equivalence" => {
            let output = output.context("--output is required")?;
            run_producer_equivalence(&output, elf.as_deref(), input.as_deref())
        }
        "pasta-fp-check" => {
            let output = output.context("--output is required")?;
            runtime.block_on(run_pasta_fp_check(&output))
        }
        "pipeline-cpu" => {
            let output = output.context("--output is required")?;
            runtime.block_on(run_pipeline_cpu(&output))
        }
        "pasta-fuzz" => {
            let output = output.context("--output is required")?;
            run_pasta_fuzz(&output)
        }
        "witness-digest" => {
            let report = witness_digest()?;
            if let Some(path) = output {
                write_json(path, &report)?;
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        "registration" => {
            let report = registration_report()?;
            if let Some(path) = output {
                write_json(path, &report)?;
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
        other => bail!("unknown mode {other}"),
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use sp1_hypercube::{septic_digest::SepticDigest, UntrustedConfig};

    // Synthetic public values exercise validation code only; no proof is constructed.
    fn fixture() -> (MachineVerifyingKey<SP1GlobalContext>, Pv) {
        let vk = MachineVerifyingKey {
            pc_start: [SP1Field::zero(); 3],
            initial_global_cumulative_sum: SepticDigest::zero(),
            preprocessed_commit: Default::default(),
            untrusted_config: UntrustedConfig::zero(),
        };
        let mut pv = Pv::default();
        pv.is_first_execution_shard = SP1Field::one();
        pv.is_execution_shard = SP1Field::one();
        pv.initial_timestamp = [
            SP1Field::zero(),
            SP1Field::zero(),
            SP1Field::zero(),
            SP1Field::one(),
        ];
        pv.last_timestamp = [
            SP1Field::zero(),
            SP1Field::zero(),
            SP1Field::zero(),
            SP1Field::from_canonical_u32(2),
        ];
        pv.next_pc = [
            SP1Field::from_canonical_u64(sp1_core_executor::HALT_PC),
            SP1Field::zero(),
            SP1Field::zero(),
        ];
        pv.last_init_addr[0] = SP1Field::one();
        pv.last_finalize_addr[0] = SP1Field::one();
        pv.commit_syscall = SP1Field::one();
        pv.commit_deferred_syscall = SP1Field::one();
        pv.global_cumulative_sum = SepticDigest::zero();
        (vk, pv)
    }
    fn rejects(vk: &MachineVerifyingKey<SP1GlobalContext>, pv: Pv, expected: &str) {
        let err = check_public_value_chain(vk, &[pv]).unwrap_err();
        assert_eq!(err.check, expected, "{err}");
    }
    #[test]
    fn native_public_value_control_is_structurally_valid() {
        let (vk, pv) = fixture();
        check_public_value_chain(&vk, &[pv]).unwrap();
    }
    #[test]
    fn nonzero_exit_rejects_at_the_new_canonical_check() {
        let (vk, mut pv) = fixture();
        pv.exit_code = SP1Field::one();
        rejects(&vk, pv, "canonical_successful_exit");
    }
    #[test]
    fn exceptional_global_addition_is_a_rejection_not_a_panic() {
        let (vk, mut pv) = fixture();
        pv.global_cumulative_sum = SepticDigest::starting_digest();
        rejects(&vk, pv, "global_cumulative_sum_exception");
    }
    #[test]
    fn chain_checks_pin_the_rejecting_layer() {
        let (vk, pv) = fixture();
        let mut x = pv;
        x.initial_timestamp[0] = SP1Field::one();
        rejects(&vk, x, "timestamp_chain");
        let mut x = pv;
        x.pc_start[0] = SP1Field::one();
        rejects(&vk, x, "pc_start_matches_vk");
        let mut x = pv;
        x.next_pc = [SP1Field::zero(); 3];
        rejects(&vk, x, "execution_halted");
        let mut x = pv;
        x.is_first_execution_shard = SP1Field::from_canonical_u32(2);
        rejects(&vk, x, "first_execution_shard_boolean");
        let mut x = pv;
        x.is_first_execution_shard = SP1Field::zero();
        rejects(&vk, x, "first_execution_shard_present");
        let mut x = pv;
        x.previous_init_addr[0] = SP1Field::one();
        rejects(&vk, x, "memory_init_chain");
        let mut x = pv;
        x.previous_finalize_addr[0] = SP1Field::one();
        rejects(&vk, x, "memory_finalize_chain");
        let mut x = pv;
        x.is_untrusted_programs_enabled = SP1Field::one();
        rejects(&vk, x, "untrusted_programs_flag");
        let mut x = pv;
        x.commit_syscall = SP1Field::zero();
        rejects(&vk, x, "commit_syscall_called");
        let mut x = pv;
        x.commit_deferred_syscall = SP1Field::zero();
        rejects(&vk, x, "commit_deferred_syscall_called");
        assert_eq!(
            check_public_value_chain(&vk, &[]).unwrap_err().check,
            "empty_proof"
        );
    }
    #[test]
    fn shared_ordering_keeps_every_tied_item_and_decodes_addresses() {
        let (_, pv) = fixture();
        let mut high = pv;
        high.previous_init_addr = [SP1Field::zero(), SP1Field::one(), SP1Field::zero()];
        let mut low = pv;
        low.previous_init_addr = [
            SP1Field::from_canonical_u32(65535),
            SP1Field::zero(),
            SP1Field::zero(),
        ];
        let items = vec![(high, 0), (low, 1), (high, 2), (low, 3)];
        let out =
            sp1_core_machine::utils::order_execution_items::<SP1Field, _>(items, |item| &item.0);
        assert_eq!(
            out.iter().map(|x| x.1).collect::<Vec<_>>(),
            vec![1, 3, 0, 2]
        );
        assert_eq!(out.len(), 4);
    }
    #[cfg(feature = "mprotect")]
    #[test]
    fn user_configuration_checks_are_bound_to_the_key() {
        let (vk, pv) = fixture();
        let mut x = pv;
        x.trap_context[0][0] = SP1Field::one();
        rejects(&vk, x, "trap_context_matches_vk");
        let mut x = pv;
        x.untrusted_memory[0][0] = SP1Field::one();
        rejects(&vk, x, "untrusted_memory_matches_vk");
        let mut x = pv;
        x.enable_trap_handler = SP1Field::one();
        rejects(&vk, x, "trap_handler_matches_vk");
    }
}

/// Differential checks for the batched slope-denominator inversion in the
/// Weierstrass add chip: the batched trace and byte lookups must equal what
/// unbatched row-by-row population produces, inside one binary.
#[cfg(test)]
mod batch_inversion_tests {
    use super::*;
    use sp1_core_executor::events::ByteRecord;
    use sp1_core_machine::operations::field::field_op::{batch_mod_inverse, mod_inverse};
    use sp1_core_machine::SupervisorMode;
    use sp1_curves::EllipticCurve;

    #[test]
    fn executor_curve_operations_match_generic_and_reference() {
        let report = fuzz_executor(&mut FuzzRng(0x7e57), 16).unwrap();
        for curve in ["vesta", "pallas"] {
            assert_eq!(report[curve]["random_cases"], 16);
        }
    }

    #[test]
    fn batch_inverse_matches_single_inversions_with_zeros() {
        for curve in [Curve::Vesta, Curve::Pallas] {
            let m = modulus(curve);
            let mut values: Vec<BigUint> = (1u32..200)
                .map(|i| (BigUint::from(i).pow(37) + BigUint::from(i)) % &m)
                .collect();
            // Zeros and multiples of the modulus have no inverse; they must
            // come back as zero without disturbing their neighbours.
            values.insert(0, BigUint::zero());
            values.insert(50, m.clone());
            values.push(&m * 3u32);
            values.push(&m - 1u32);
            let batched = batch_mod_inverse(&values, &m);
            assert_eq!(batched.len(), values.len());
            for (value, inverse) in values.iter().zip(&batched) {
                let reduced = value % &m;
                if reduced.is_zero() {
                    assert!(inverse.is_zero());
                } else {
                    assert_eq!(*inverse, mod_inverse(&reduced, &m));
                    assert_eq!((&reduced * inverse) % &m, BigUint::one());
                }
            }
            assert!(batch_mod_inverse(&[], &m).is_empty());
            assert!(batch_mod_inverse(&[BigUint::zero()], &m)[0].is_zero());
            assert_eq!(batch_mod_inverse(&[BigUint::one()], &m), vec![BigUint::one()]);
        }
    }

    /// Population of every event row the unbatched way, then compare against
    /// the chip's own (batched) trace rows and dependency byte lookups.
    fn assert_add_chip_matches_unbatched<E: EllipticCurve + sp1_curves::weierstrass::WeierstrassParameters>(
        record: &ExecutionRecord,
        syscall: SyscallCode,
    ) -> usize {
        let chip = WeierstrassAddAssignChip::<E, SupervisorMode>::new();
        let width = num_weierstrass_add_cols_supervisor::<E::BaseField>();
        let events = record.get_precompile_events(syscall);
        if events.is_empty() {
            return 0;
        }
        let trace = MachineAir::<SP1Field>::generate_trace(&chip, record, &mut ExecutionRecord::default());
        let mut deps = ExecutionRecord::default();
        MachineAir::<SP1Field>::generate_dependencies(&chip, record, &mut deps);

        let mut reference_lookups = ExecutionRecord::default();
        for (idx, (_, op)) in events.iter().enumerate() {
            let event = match op {
                PrecompileEvent::VestaAdd(event) | PrecompileEvent::PallasAdd(event) => event,
                _ => unreachable!(),
            };
            let mut row = vec![SP1Field::zero(); width];
            let mut blu = Vec::new();
            let cols: &mut WeierstrassAddAssignCols<SP1Field, E::BaseField, SupervisorMode> =
                row.as_mut_slice().borrow_mut();
            WeierstrassAddAssignChip::<E, SupervisorMode>::populate_row(event, cols, &mut blu);
            assert_eq!(
                &trace.values[idx * width..(idx + 1) * width],
                row.as_slice(),
                "row {idx} differs between batched and unbatched population"
            );
            reference_lookups.add_byte_lookup_events(blu);
        }
        assert_eq!(
            deps.byte_lookups, reference_lookups.byte_lookups,
            "dependency byte lookups differ between batched and unbatched population"
        );
        events.len()
    }

    #[test]
    fn batched_add_witness_equals_unbatched_rows_and_lookups() {
        // 300 additions per curve spans several 64-row trace chunks and the
        // dependency chunks, with distinct operands in every call.
        for op in OPS.iter().filter(|op| op.kind == Kind::Add) {
            let points = multiples(op.curve, 601);
            let calls: Vec<Call> = (0..300)
                .map(|i| make_call(*op, points[i].clone(), Some(points[300 + i].clone())))
                .collect::<Result<_>>()
                .unwrap();
            let (program, _) = build_program(&calls).unwrap();
            let (records, _) =
                generate_records::<SP1Field>(program, SP1Stdin::new(), core_opts(), [0; 4]).unwrap();
            let checked: usize = records
                .iter()
                .map(|record| match op.curve {
                    Curve::Vesta => assert_add_chip_matches_unbatched::<Vesta>(record, op.syscall),
                    Curve::Pallas => assert_add_chip_matches_unbatched::<Pallas>(record, op.syscall),
                })
                .sum();
            assert_eq!(checked, 300, "{}: every addition must be checked", op.id);
        }
    }
}
