# Pasta AIR verification plan: Lean proofs and differential fuzzing

Status: plan, 2026-11-10. The first fuzzing layer exists (`pasta-fuzz`, below);
no Lean development exists yet. Nothing here changes the retained verifier.

## Why these chips, and why both methods

The SP1 fork adds eight AIR variants: Pallas and Vesta, add and double, trusted
and user mode. They are the only custom constraint code between a Mina proof
and the boolean the guest commits. An under-constrained column produces proofs
that verify, and an honest prover never notices, because it never exercises
the gap. Tests sample the input space and fuzzing searches it, but neither can
show a gap is absent. A machine-checked proof can.

The approach follows Ragu's formal-verification work for Project Tachyon
(Ragu book, part IV): extract the exact constraints from Rust, prove theorems
about an independent Lean reimplementation, and bind the two with a
deterministic fingerprint plus a randomized evaluation check in CI. It differs
where SP1 differs from Ragu: our constraints are per-row AIR polynomials over
KoalaBear plus interaction (lookup and memory) arguments, not gate circuits.

## Scope

The eight variants reduce to two AIR shapes, instantiated at two moduli:

| Shape | Columns that matter | Trust modes |
| --- | --- | --- |
| `WeierstrassAddAssignCols` | 10 `FieldOpCols` (incl. `inverse_check`), 2 `FieldLtCols` | User mode adds read and write page-protection columns |
| `WeierstrassDoubleAssignCols` | 11 `FieldOpCols`, 2 `FieldLtCols`, no inverse check | User mode adds write page-protection columns |

Every arithmetic column is built from two gadgets, so they are the first
verification targets:

- **`FieldOpCols`**: 32 byte limbs for the result and the carry, `u16` witness
  limbs, and the identity `op - result - carry * p = (X - 256) * witness` on
  the limb polynomials (`eval_with_polynomials`, `eval_field_operation`).
- **`FieldLtCols`**: the canonical-output range check `x3, y3 < p`.

The Poseidon permutation chips added on 2026-11-10 (`PastaPoseidonRoundCols`,
`PastaPoseidonControlCols`, at both Pasta base fields and both trust modes)
widen this scope: the round chip composes `FieldOpCols` (four multiplications
for `x^7`) with a four-term `FieldInnerProductCols` whose witness offset is
`2^15`, and its 55 rows are linked by a chip-local lookup chain rather than by
row algebra. The limb-identity lemma must therefore be proved at the larger
offset and for the n-term inner product, and the chain becomes a stated
interaction assumption like the memory bindings below. Their harness mode is
`pasta-poseidon-check` (`docs/optimizations/04-poseidon.md`).

Out of scope for proofs: SP1's lookup and memory arguments, the recursion
verifier, the executor, and the guest's MSM logic. Interactions are treated as
stated assumptions (below), and argued by SP1's own soundness.

## Properties to prove

For each shape, at each Pasta modulus, against explicit input assumptions:

- **Soundness.** Any row that satisfies the AIR's algebraic constraints, with
  every byte and `u16` lookup column in range, has `is_real = 0`, or its output
  limbs encode `P + Q` (add) or `2P` (double) over the base field, as canonical
  residues. Inputs are the operand limbs that the memory interactions bind.
- **Completeness.** For inputs meeting the preconditions, the honest witness
  generator's row satisfies every constraint. This guards liveness, not
  soundness, and is cheaper to prove.

Lemmas, in dependency order:

1. **Limb-identity lemma.** With result and carry limbs in `[0, 256)` and
   witness limbs offset into `[0, 2^16)`, each coefficient equation over
   KoalaBear implies the same equation over the integers. This needs explicit
   coefficient bounds: 32 limbs of 8 bits give product coefficients below
   `32 * 255^2`, far under KoalaBear's `2^31 - 2^24 + 1`. This is where SP1-style
   emulated arithmetic hides real bugs, so it comes first.
2. **Field-op lemma.** The integer identity implies `result ≡ a op b (mod p)`
   for add, sub and mul, and `result * b ≡ a (mod p)` for div.
3. **Range lemma.** `FieldLtCols` implies `x < p`.
4. **Chip lemmas.** Composition gives the slope, `x3` and `y3` relations, hence
   the group law, under the shape's preconditions.

**The double chip's precondition is the first open question.** It has no
inverse check, so `slope * 2y = 3x^2` leaves the slope unconstrained when
`y = 0` and `x = 0`. Pasta curves have odd prime order, so no point on the
curve has `y = 0`, but the AIR never checks that its input is on the curve.
The proof must state `2y ≠ 0` as an assumption and show the guest ABI
discharges it, or the chip gains an inverse-check column. The r9 forged-slope
tests and the Miza review found this gap from the witness side.

## Architecture: tiny trusted extraction, structured Lean model

Ragu weighed exporting high-level structure against exporting a concrete
low-level trace, and chose the latter. That keeps the trusted exporter small,
at the price of proving facts about one concrete instance at a time. With
exactly four arithmetic instances (two shapes, two moduli) the same choice
fits here.

1. **Extract (Rust).** For each chip, run `Air::eval` under SP1's
   `SymbolicAirBuilder` (`slop_uni_stark::get_symbolic_constraints`) and
   `InteractionBuilder`. Emit every constraint as a polynomial over named
   columns in a canonical normal form: sorted monomials, zero terms dropped,
   fixed-width or length-prefixed tokens. Emit every interaction with its
   kind, multiplicity expression and message expressions. Hash the encoding
   (SHA-256) into one fingerprint per chip, as Ragu does.
2. **Model (Lean 4 + Mathlib).** Write a structured reimplementation: the
   `FieldOpCols` and `FieldLtCols` gadgets as Lean definitions parameterized
   by the modulus, composed into the two shapes. Prove the lemmas above about
   the reimplementation, over `ZMod p_koala` with explicit limb bounds.
   Ragu uses the Clean framework; Clean's encoding does not support lookups,
   which every SP1 range check uses. Start in plain Lean with a minimal
   row-constraint model and revisit Clean once its lookup support is known.
3. **Bind (CI).** Two complementary checks, both from Ragu:
   - *Fingerprint equivalence.* Lean computes the normal-form encoding of its
     reimplementation's constraints at the concrete modulus; CI compares it
     with the Rust fingerprint. A match means the reimplementation emits the
     same constraints, modulo polynomial normalization and SHA-256 collisions.
   - *Randomized evaluation.* From one CI-drawn seed, Rust evaluates the real
     chip's constraints at random column assignments, and Lean evaluates its
     model at the same points. By Schwartz–Zippel, disagreement at two
     independent points is overwhelmingly likely if the systems differ. This
     covers any gap in the extractor's own model.
4. **Interaction assumptions.** Each lookup becomes a named hypothesis on the
   columns it ranges (`byte`, `u16`), and each memory/syscall interaction a
   hypothesis that binds the operand limbs to the guest's memory words.
   List the axioms with `#print axioms` and keep `sorry` out of CI
   (`--wfail`), as Ragu does.

## Differential fuzzing (runs now, and stays after the proofs)

Fuzzing finds bugs fast and keeps finding regressions after the proofs land.
Layers F1 to F3 are implemented as the harness's `pasta-fuzz` mode; F4 to F6
are planned.

| Layer | What is compared | Status |
| --- | --- | --- |
| F1 executor | Host curve operations (`ec_add`, `ec_double`) vs the generic formulas vs the independent integer reference, plus degenerate inputs (`P + (-P)`, `P = Q`, `y = 0`, noncanonical coordinates) that must behave identically on both paths, panics included | Implemented |
| F2 witness | Random operands through a real program: every executor result and every trace row's `x3`, `y3` limbs vs the reference; every honest row satisfies the AIR | Implemented |
| F3 column sweep | Every column of an honest row perturbed (+1 and a random nonzero step); columns whose perturbation the algebraic constraints accept are listed by field | Implemented |
| F4 lookup-aware mutation | F3's survivors re-run through the full CPU proof of the small Pasta guest, so byte, memory and syscall arguments are evaluated too | Planned; reuse the r123 proof matrix |
| F5 Rust–Lean evaluation | The randomized evaluation check above, also run with fuzzed rather than random points | Planned, with the Lean model |
| F6 cross-curve | The same shapes at secp256k1 and BN254, where SP1 upstream already tests, to separate fork bugs from upstream ones | Planned |
| P1 Poseidon host | The fork's fixed-limb permutation vs its BigUint reference vs a reference evaluated from the proof-systems parameter export, on random canonical, random noncanonical (`>= p`) and edge inputs (`0`, `p − 1`, `p`, `p + 1`, `2^256 − 1`); outputs canonical; panics counted | Implemented |
| P2 Poseidon witness | Random states through a real program: every executor output, and every round row's clock, pointer, index, input and output against the reference's 56 intermediate states; honest rows of both chips satisfy the AIRs | Implemented |
| P3 Poseidon column sweep | F3 on four round rows (first, random middle, last, and the next permutation's first) and both controller rows; survivors must lie in the interaction-bound fields | Implemented |
| P4 Poseidon lookup-aware mutation | Random single-cell mutations of real rows, half of them inside interaction-bound fields: each must fail the row algebra, leave the round chain unbalanced, or sit in the one memory-only field (`final_memory_access`); plus a round row re-labelled to another round and two permutations' outputs swapped | Implemented (chain only; memory and syscall arguments are not evaluated) |

Every fuzz run takes a seed (`PASTA_FUZZ_SEED`, decimal or `0x`-prefixed,
printed and recorded) and an iteration count (`PASTA_FUZZ_ITERS`). Run F1 to F3 on every fork change; run a
longer seed sweep nightly. The recorded survivor set for arithmetic fields is
empty, and the sweep now fails on any new survivor (checked on seed
`0x5eed0004`).

### First run (2026-11-10)

**Correction (same day):** the seed parser accepted decimal only, so the hex
seeds below silently fell back to the default `0x5eed0001`. This run was one
seed, not three, and the iteration counts differed as stated. The parser now
accepts `0x` values and fails on anything it cannot parse; the second run
below is the first with distinct seeds, and its curve layers passed on all of
them.

Three seeds (`0x5eed0001` with 32 iterations, `0x5eed0002` and `0x5eed0003`
with 64) on the candidate fork with batched witness inversion and the
fixed-limb executor path. CPU only, no proof.

- **F1 executor:** every random add and double on both curves matched the
  generic path and the reference. On all six degenerate inputs per curve, the
  fast and generic paths agreed, panics included: `P = Q` panics on both, and
  Pallas noncanonical inputs panic on both (the generic big-integer
  subtraction underflows), because noncanonical inputs never leave the generic
  path.
- **F2 witness:** 544 operations across the four chips. Every executor result
  and every trace row's `x3`, `y3` matched the reference, and every honest row
  satisfied the AIR.
- **F3 column sweep:** zero surviving columns in all four chips (1,599 add and
  1,591 double columns). Even the clock and pointer columns are tied into
  algebraic relations (memory-timestamp ordering, address arithmetic), not
  only into interactions. The same evaluator accepts the r9 forged double row,
  so it can report a satisfied trace.

What F3 does not show: a single-column perturbation cannot find coordinated
multi-column forgeries. The double chip's slope forgery at `(0, 0)` needs a
crafted input and a matching slope, and F3 does not see it. That is what the
Lean soundness theorem, and its `2y ≠ 0` precondition, are for.

### Second run (2026-11-10): Poseidon layers, distinct seeds

Seeds `0x5eed0005`, `0x5eed0006`, `0x5eed0007`, 32 iterations each, SP1 fork
`a418319a0`, about two minutes a run. Curve layers F1 to F3 passed on all
three with zero surviving columns. Poseidon layers, both fields, every seed:

- **P1:** 68 or 69 inputs per field and seed (32 random canonical, 31 or 32
  random noncanonical, 5 edge cases); host, fork reference and export
  reference agreed on every input, every output canonical, no panics.
- **P2:** 32 permutations per field; 1,760 round rows checked against the
  reference's intermediate states; all honest rows satisfied both AIRs.
- **P3:** 8,192 round cells and 1,014 controller cells perturbed per field;
  survivors only in `clk_high`, `clk_low`, `state_addr` (round) and
  `initial_memory_access`, `final_memory_access`, `final_state` (controller),
  186 of 192 final-state bytes among them, which the chain and the memory
  write bind.
- **P4:** 384 mutations in all: 333 rejected by the row algebra, 45 by the
  chain balance, 6 in `final_memory_access`, 0 unexplained. The re-labelled
  round row and the swapped outputs were rejected on every seed.

What P4 does not show: `final_memory_access` (previous timestamps and values
of the written state) and the controller's syscall columns are bound by SP1's
memory and syscall arguments, which this harness does not evaluate; a real
proof does. Evidence: [summary](../benchmarks/pasta-poseidon-fuzz-sp1-6.10.json).

## Phases

| Phase | Deliverable | Exit criterion | Effort |
| --- | --- | --- | --- |
| 0 | `pasta-fuzz` F1–F3 and Poseidon P1–P4; survivor sets recorded | Clean run on three distinct seeds | Done locally (second run); CI wiring open |
| 1 | Rust extractor and fingerprints for the four instances | Stable fingerprint across two builds; any constraint edit changes it | 1 week |
| 2 | Lean limb-identity, field-op and range lemmas | No `sorry`; axioms listed | 2–3 weeks |
| 3 | Lean add and double models; fingerprint match; randomized check in CI | Both checks pass in CI | 2 weeks |
| 4 | Soundness and completeness for add; double with its `2y ≠ 0` decision | Theorems proved, or a constraint added and re-proved | 2–3 weeks |
| 5 | F4 and F6; user-mode page-protection columns | Lookup-aware survivors explained | 1–2 weeks |

Effort assumes one engineer familiar with Lean and Mathlib; it is an estimate,
not a commitment.

## Trust boundary, stated once

The proofs will say: if the lookup and memory arguments enforce what they claim
(ranges, operand binding), then every accepted Pasta row computes the group law
on canonical residues. They will not say anything about SP1's prover, its
recursion, the executor, or the guest's use of the syscalls. The fingerprint
and randomized checks trust SHA-256 and the two encoders. Report results in
exactly those terms.
