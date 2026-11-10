# Pasta AIR verification plan: Lean proofs and differential fuzzing

Status: plan, 2026-11-10. The fuzzing layers exist (`pasta-fuzz` and the two
native harnesses, below); no Lean development exists yet. Nothing here changes
the retained verifier.

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

The fixed-modulus field chip the Pasta syscalls also use is SP1's own
`FpOpChip` at the two Pasta moduli, with no new constraint code, so it is out
of scope for the proofs. It is in scope for the fuzzing, because what the fork
chose is which modulus and which operands reach it: layers FP1 to FP4 cover
that, including what the chip does with an operand the executor would reduce.

Out of scope for proofs: SP1's lookup and memory arguments, the recursion
verifier, the executor, and the guest's MSM logic. Interactions are treated as
stated assumptions (below), and argued by SP1's own soundness. The fuzzing is
narrower about one of them: a mutation that changes a chip's contribution to
the memory, page-protection or syscall argument, or asks a fixed table for a
message it does not hold, is rejected without that assumption.

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
Every layer below except F5 and F6 is implemented. The curve layers F1 to F4,
the Poseidon layers P1 to P4, the fixed-modulus field layers FP1 to FP4, the
multi-row and forgery layers C1 and C2 and the executor differential D1 and D2
are the harness's `pasta-fuzz` mode; V1 and V2 are the native verifier harness
(`crates/cycle-runner/src/bin/verifier-fuzz.rs`) and A1 a native test
(`crates/pickles-verifier/tests/canonical_words_fuzz.rs`), neither of which
touches an AIR.

F4 is implemented, but not the way this plan proposed it. Rather than re-running
survivors through a full CPU proof, the harness compares the chip's own
contribution to the memory, page-protection and syscall arguments before and
after a mutation. Those arguments balance over every chip of a shard, so a
changed contribution cannot be absorbed by an otherwise honest shard: it is a
rejection, established without generating a proof. The honest balance itself is
checked per run over every participating chip.

Byte lookups are asked a different question, because the byte and range tables'
multiplicity columns are supplied by the prover while their rows come from a
preprocessed trace. Balance there proves nothing; membership does. The harness
enumerates the message set those tables can serve, once per run from their
preprocessed traces, and a mutation that makes a row request a message no table
holds is a rejection at any multiplicity. This is the only thing that binds the
low-byte columns of a memory word: `U16toU8Operation` keeps the low byte of
each 16-bit limb as a column and derives the high byte as `(limb - low) / 256`,
so a single-cell mutation of a low byte moves both bytes, leaves the limb
intact, and changes no memory message. One of the two bytes must then leave
`[0, 256)`, which is what the range check refuses.

A prover that also rewrites the chips on the other side of an argument is
outside what any of this measures, and remains the planned full-proof work.

| Layer | What is compared | Status |
| --- | --- | --- |
| F1 executor | Host curve operations (`ec_add`, `ec_double`) vs the generic formulas vs the independent integer reference, plus degenerate inputs (`P + (-P)`, `P = Q`, `y = 0`, noncanonical coordinates) that must behave identically on both paths, panics included | Implemented |
| F2 witness | Random operands through a real program: every executor result and every trace row's `x3`, `y3` limbs vs the reference; every honest row satisfies the AIR | Implemented |
| F3 column sweep | Every column of an honest row perturbed (+1 and a random nonzero step); columns whose perturbation the algebraic constraints accept are listed by field | Implemented |
| F4 argument-aware mutation | Whether a mutation changes the chip's own contribution to the memory, page-protection or syscall argument, which the honest remainder of the shard can no longer balance, or makes a row request a byte-lookup message no fixed table holds | Implemented without a proof (see above); the full-proof variant stays open |
| F5 Rust–Lean evaluation | The randomized evaluation check above, also run with fuzzed rather than random points | Planned, with the Lean model |
| F6 cross-curve | The same shapes at secp256k1 and BN254, where SP1 upstream already tests, to separate fork bugs from upstream ones | Planned |
| P1 Poseidon host | The fork's fixed-limb permutation vs its BigUint reference vs a reference evaluated from the proof-systems parameter export, on random canonical, random noncanonical (`>= p`) and edge inputs (`0`, `p − 1`, `p`, `p + 1`, `2^256 − 1`); outputs canonical; panics counted | Implemented |
| P2 Poseidon witness | Random states through a real program: every executor output, and every round row's clock, pointer, index, input and output against the reference's 56 intermediate states; honest rows of both chips satisfy the AIRs | Implemented |
| P3 Poseidon column sweep | F3 on four round rows (first, random middle, last, and the next permutation's first) and both controller rows; survivors must lie in the interaction-bound fields | Implemented |
| P4 Poseidon argument-aware mutation | Random single-cell mutations of real rows, half of them inside interaction-bound fields: each must fail the row algebra, leave the round chain unbalanced, or change the chip's contribution to an argument; plus a round row re-labelled to another round and two permutations' outputs swapped | Implemented; the memory-only class of the first two runs is now checked rather than deferred |
| FP1 fixed-modulus executor | The `PALLAS_FP_*` / `VESTA_FP_*` syscalls against a BigUint reference on random canonical, edge and noncanonical operands, through a real program that also compares each result in-guest | Implemented |
| FP1b noncanonical operands | The executor reduces its operands and `FpOpChip` does not, so a noncanonical operand's honest row may have no satisfying witness. Which operations do is reported per operation, and a row that does satisfy the AIR must carry the reduced result | Implemented |
| FP2 fixed-modulus witness | Every real row's result limbs against the executor and the reference; honest rows satisfy the AIR; no padding row is real | Implemented |
| FP3 fixed-modulus column sweep | F3 on the first, a random middle and the last real row and on the first padding row, with every survivor classified against the arguments and the fixed tables | Implemented |
| FP4 fixed-modulus mutation | Random single-cell mutations of real rows, half inside the fields the row algebra never constrains; each must fail the algebra or change an argument | Implemented |
| C1 curve multi-row sweep | F3 across four rows of a six-call trace instead of the first row of a one-call trace, with survivors classified against the arguments and padding rows separated | Implemented |
| C2 randomized degenerate forgeries | The double chip's `y = 0` rows at random x with random forged slopes, and the add chip's equal-x rows at `q = p`, `q = -p` and a random `q.y`: all must be rejected, except the one input the relation leaves free (`x = y = 0`), which must still be caught by the memory argument when written over an honest trace | Implemented |
| D1 executor argument table | Each syscall family under malformed register arguments (misaligned pointers, addresses outside the valid range, a nonzero second argument where the ABI requires zero), with the minimal and tracing executors required to reach the same verdict, and the same words in the well-formed case | Implemented |
| D2 executor random offsets | The same comparison at random pointer offsets, plus plain store-and-load controls that attribute a divergence to the address rather than the syscall | Implemented |
| V1 verifier typed mutation | Every typed mutator once and then random draws over one field of a `VerifiableProof` — statement field, deferred challenge, evaluation, commitment, IPA response — with a random field element or a negated point; the native verifier must reject, over all nine fixtures, except for the one field it reads nowhere | Implemented |
| V2 verifier single-bit mutation | One random bit of the serialized proof flipped; most do not deserialize, which is counted apart, and every one that does must be rejected unless restoring the ignored field reproduces the honest bytes | Implemented |
| A1 canonical arithmetic | The canonical Pasta residues against arkworks on boundary classes and random operands: the conditional subtraction of `add_assign`, the `pow7` schedules and the deferred three-term inner products. Host products are arkworks, so the subject is the logic around them | Implemented |

Every `pasta-fuzz` run takes a seed (`PASTA_FUZZ_SEED`, decimal or
`0x`-prefixed, printed and recorded) and an iteration count
(`PASTA_FUZZ_ITERS`); `verifier-fuzz` takes `--seed`, `--typed` and `--bytes`.
Run the harness on every fork change and a longer seed sweep nightly. Every
sweep now fails on a cell that neither a constraint, an argument nor a fixed
table rejects, and the differential fails on a divergence its controls do not
attribute to the address.

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

What P4 did not show at the time: `final_memory_access` (previous timestamps
and values of the written state) and the controller's syscall columns are bound
by SP1's memory and syscall arguments, which this run did not evaluate. The
third run does, and the class is checked there rather than deferred. Evidence:
[summary](../benchmarks/pasta-poseidon-fuzz-sp1-6.10.json).

### Third run (2026-11-10): the argument channel, the field chip and the executors

Four runs on the same fork pins as the second: seeds `0x5eed0301`,
`0x5eed0302` and `0x5eed0303` at 32 iterations, and `0x5eed0202` at 16, which
is the seed whose low-byte cell prompted the fixed-table check and the only one
of the four that exercises it. 217 to 236 seconds a run. The curve layers F1 to
F3 and the Poseidon layers P1 to P3 passed unchanged; what follows is the new
material. The fixed byte and range tables contribute 459,007 distinct messages,
enumerated once per run from their preprocessed traces.

- **F4, the argument channel.** The honest shard of every layer balances its
  memory, page-protection and syscall arguments with no open message, over the
  chips the shard includes, which for these synthetic programs are the chip
  under test, the syscall chip and the local memory chip. Every mutation that
  survived the row algebra was then classified, and none was left unexplained.
- **P4 with the argument channel.** 448 mutations across the four runs: 377
  rejected by the row algebra, 61 by the round chain and 10 by the memory
  argument, none unexplained. The memory-only class of the second run is what
  those last ten are, and it is now checked rather than deferred: such a
  mutation has to change the chip's own contribution to the memory argument.
- **FP1 and FP1b.** 474 Pallas and 492 Vesta field operations through a real
  program, on random canonical, edge and noncanonical operands; the executor
  matched the reference on every one. For noncanonical operands the chip and
  the executor part company: the executor reduces and the chip does not, so
  the honest row may have no satisfying witness, the carry having outgrown its
  limbs. Which rows do is reported per row rather than ruled: in these runs
  most subtractions had none and most additions and multiplications had one.
  Every row that satisfies the AIR is required to carry the reduced result, so
  this is a liveness boundary, like the curve chips' noncanonical inputs, and
  not a second meaning for the same memory words.
- **FP2.** 114 real rows per field and run, 66 on the 16-iteration seed, every
  result read back from its limbs and matched against both the executor and
  the reference; honest rows satisfied the AIR and no padding row was real.
- **FP3 and FP4.** 1,224 cells per field swept over four rows, and 448
  mutations in all. Every surviving cell of the sweep was on the padding row,
  where the row is not real and contributes to nothing. Of the mutations, 447
  were rejected by the row algebra and one by a fixed table: a low byte of a
  memory limb, which `U16toU8Operation` pairs with a derived high byte, so
  moving it alone leaves the limb and every memory message intact and only the
  range check refuses it.
- **C1.** 6,396 add and 6,364 double cells swept over four rows of a six-call
  trace. Every surviving cell was on the padding row: 210 of them for the add
  chips and 140 for the double chips, in every run.
- **C2.** Eight forged rows per operation with random inputs and random
  slopes. The add chip's `inverse_check` rejects every equal-x row, whether the
  second operand is `p`, `-p` or off the curve. The double chip rejects every
  `y = 0` row with `x != 0`, where the numerator `3x^2` does not vanish, and
  accepts the one case where it does, `x = y = 0`, exactly as the fixed suite
  records. Written over an honest trace, that forgery changes the claimed
  input in memory, so the memory argument rejects it; the algebraic gap and
  the `2y != 0` precondition are unchanged.
- **D1 and D2.** 68 table cases per run, with 32 random pointer offsets (16
  on the 16-iteration seed) and 4 plain-store controls: 74 to 90
  agreements, and two divergence classes, each attributed by a control.

  A pointer below SP1's 2^16 data floor: the minimal executor completes and
  the tracing replay panics inverting zero. A plain store and load at the same
  address, with no syscall in the program, reproduces it, while the same
  control inside the valid range runs on both executors, so the divergence
  belongs to the address and not to the syscall.

  A malformed operand value, on the two double chips only. For the all-zero
  encoding the minimal executor's fixed-limb path returns zero for the inverse
  of zero and completes, while record generation panics in the witness
  generator's division. The add chips do not diverge, because their
  batched-inversion path populates the same columns without asserting, so the
  record is built and the row is merely unsatisfiable. These are the degenerate
  inputs the invalid-input suite pins. The table now reaches both of them
  deliberately -- the all-zero encoding and a coordinate above the modulus --
  at a well-formed pointer, and the operand pages are filled with repeated
  valid words so that a pointer shifted by whole operands still reads a
  well-formed value. Shifting by whole words instead rotates the operand into a
  malformed coordinate, and those divergences are attributed to the value as
  well: a rotation found this class before it had a case of its own, through a
  witness-generator subtraction that underflows for some coordinates and not
  others.

  Both classes are programs that execute but cannot be traced, hence cannot be
  proved; neither is a soundness question. Misaligned pointers, addresses above
  the range and a nonzero second argument where the ABI requires zero stop
  both executors alike.
- **V1 and V2.** Nine fixtures, 288 typed mutations and 576 single-bit
  mutations of the serialized proof, seed `0x5eed0304`, 413 seconds. Of the
  typed mutations, 271 were rejected, 3 found no such field in their fixture
  and 14 were accepted; of the bit flips, 197 did not deserialize, 375 were
  rejected and 4 were accepted. The verifier never panicked. Every acceptance
  was a mutation of `p_eval0_chunks`, and that is the one thing this layer
  found: the field is a copy of `prev_evals.public_evals.zeta` that the host
  conversion derives from it, and no part of the verifier, the guest, the
  attestation or the statement digest reads it. The proof's public-input
  evaluation is still bound, through the copy that is used; this one is dead
  weight in the guest's input. The harness expects those acceptances now,
  counts them apart, requires the expectation to be exercised, and fails if any
  other field turns out to be ignored. Across the run the typed layer drew 67
  distinct targets.
- **A1.** The canonical Pasta residues against arkworks on every ordered pair
  of nine boundary classes plus 512 random pairs and 516 triples per field:
  `add_assign`, both `pow7` schedules and all three three-term inner products
  agreed everywhere, and `from_limbs` rejected `p`, `p + 1` and `2^256 - 1`.

What the third run does not show: the byte tables bind by message membership,
which the harness checks, but a prover free to rewrite the chips on the other
side of an argument is still outside the model; that is the full-proof variant
of F4. Evidence: [pasta-fuzz expansion
summary](../benchmarks/pasta-fuzz-expansion-sp1-6.10.json) and [verifier
mutation summary](../benchmarks/verifier-mutation-fuzz.json).

## Phases

| Phase | Deliverable | Exit criterion | Effort |
| --- | --- | --- | --- |
| 0 | `pasta-fuzz` F1–F3 and Poseidon P1–P4; survivor sets recorded | Clean run on three distinct seeds | Done locally (second run); CI wiring open |
| 0b | F4, FP1–FP4, C1–C2, D1–D2 in the harness; V1–V2 and A1 natively | Clean run on three distinct seeds, every survivor and divergence classified | Done locally (third run); CI wiring open |
| 1 | Rust extractor and fingerprints for the four instances | Stable fingerprint across two builds; any constraint edit changes it | 1 week |
| 2 | Lean limb-identity, field-op and range lemmas | No `sorry`; axioms listed | 2–3 weeks |
| 3 | Lean add and double models; fingerprint match; randomized check in CI | Both checks pass in CI | 2 weeks |
| 4 | Soundness and completeness for add; double with its `2y ≠ 0` decision | Theorems proved, or a constraint added and re-proved | 2–3 weeks |
| 5 | F6 and the user-mode page-protection columns; the full-proof variant of F4 for the byte table | Cross-curve and user-mode survivors explained | 1–2 weeks |

Effort assumes one engineer familiar with Lean and Mathlib; it is an estimate,
not a commitment.

## Trust boundary, stated once

The proofs will say: if the lookup and memory arguments enforce what they claim
(ranges, operand binding), then every accepted Pasta row computes the group law
on canonical residues. They will not say anything about SP1's prover, its
recursion, the executor, or the guest's use of the syscalls. The fingerprint
and randomized checks trust SHA-256 and the two encoders. Report results in
exactly those terms.
