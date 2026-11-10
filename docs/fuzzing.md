# Fuzzing: what the layers cover, and what a clean run establishes

Status: 2026-11-10. Twenty-one layers in three harnesses, all implemented and
run locally. No layer generates a proof, uses a GPU or touches the retained
verifier; every one of them runs natively on this host in minutes.

The companion documents are [the Pasta AIR verification plan](pasta-air-verification-plan.md),
which holds the layer table in its original form, the per-run results and the
Lean work these layers do not replace, and the records
[mizu-pasta-fuzz / mizu-fuzz-expansion](optimizations/07-measurement-and-tooling.md)
and [mizu-pasta-poseidon-precompile](optimizations/04-poseidon.md).

## Why fuzz these particular things

The fork adds constraint code in three places — eight Weierstrass AIR variants
(Pallas and Vesta, add and double, trusted and user mode), the Pasta Poseidon
round and control chips, and the choice of which modulus and which operands
reach SP1's own `FpOpChip` — plus two executor implementations per syscall and
a native Pickles verifier the guest mirrors. An under-constrained column or a
field the verifier forgets to read produces proofs that verify, and an honest
prover never notices, because it never exercises the gap. These layers search
for that; they cannot show its absence, which is what the Lean plan is for.

## The three harnesses

| Harness | Subject | Layers | Runtime |
| --- | --- | --- | --- |
| `pasta-precompile-proof-6-10 --mode pasta-fuzz` | the fork's AIRs, witness generators and both executors | F1–F4, P1–P4, FP1–FP4, C1–C2, D1–D2 | 217–236 s at 32 iterations |
| `cycle-runner`'s `verifier-fuzz` binary | the out-of-circuit Pickles verifier, over all nine fixtures | V1, V2 | 413 s at 32 typed and 64 bit flips per fixture |
| `pickles-verifier`'s `canonical_words_fuzz` test | the closed canonical Pasta residues against arkworks | A1 | seconds |

## The four questions a layer asks

1. **Differential.** One implementation against another and against an
   independent reference: the executor's fixed-limb curve path against the
   generic big-integer formulas against an integer reference (F1); the fork's
   Poseidon permutation against its own BigUint reference and against a third
   evaluated from the proof-systems parameter export (P1); the field syscalls
   against a BigUint reference (FP1); the minimal executor against the tracing
   executor (D1, D2); `CanonicalPasta` against arkworks (A1).
2. **Witness agreement.** Random operands through a real program, with every
   executor result and every real trace row checked against the reference, and
   every honest row required to satisfy its AIR (F2, P2, FP2).
3. **Column sweep.** Every column of a chosen row perturbed, by +1 and by a
   random nonzero step, to see what the row algebra still accepts (F3, P3,
   FP3, C1).
4. **Mutation.** A single cell changed deliberately, often inside the fields
   the row algebra never constrains, which something must reject (P4, FP4);
   plus whole-row forgeries of the known degenerate cases (C2), a round row
   re-labelled to another round, two permutations' outputs swapped (P4), and
   typed or single-bit mutations of a serialized proof (V1, V2).

## Two channels of rejection, and why byte tables differ

SP1's `debug_constraints` evaluates one chip's row algebra and nothing else, so
a column bound only by a lookup argument looks unconstrained to a sweep. Every
surviving cell and every mutation is therefore classified against four
possibilities, and a run fails on anything that fits none of them:

- **The row algebra** rejects it.
- **An argument** rejects it: the mutation changes the chip's own contribution
  to the memory, page-protection or syscall argument. Those balance over every
  chip of a shard, so a changed contribution cannot be absorbed by an
  otherwise honest shard — a rejection, with no proof generated. The honest
  balance is checked per layer, over every participating chip, rather than
  assumed: a run requires zero open messages before it mutates anything.
- **A fixed table** cannot serve it. The byte and range tables are asked a
  different question, because their multiplicity columns are prover-supplied
  while their rows come from a preprocessed trace no record and no mutation can
  change. Balance there proves nothing; membership does. The harness enumerates
  the message set those tables can serve once per run (459,007 distinct
  messages) and a row that requests a message no table holds is rejected at any
  multiplicity.
- **The cell is inert**: it lies on a padding row, which is not real and
  contributes to nothing.

The membership check is the only thing that binds the low-byte columns of a
memory word. `U16toU8Operation` keeps the low byte of each 16-bit limb as a
column and derives the high byte as `(limb - low) / 256`, so moving a low byte
alone moves both bytes, leaves the limb intact and changes no memory message.
One of the two bytes must then leave `[0, 256)`, which is exactly what the
range check refuses.

## Coverage by subject

**Curve precompiles.** F1 covers the executor's two paths and six degenerate
inputs per curve (`P + (-P)`, `P = Q`, `y = 0`, noncanonical coordinates),
where both paths must behave identically, panics included. F2 checks every
executor result and every row's `x3`, `y3`. F3 sweeps the first row of a
one-call trace (1,599 add and 1,591 double columns); C1 sweeps four rows of a
six-call trace instead, so the sweep is no longer a statement about row 0
alone. C2 forges the two degenerate rows the fixed suite pins at one point
each: the double chip's `y = 0` rows at random x with random slopes, and the
add chip's equal-x rows at `q = p`, `q = -p` and a random `q.y`.

**Poseidon.** P1 is the three-way permutation differential over random
canonical, random noncanonical and edge inputs. P2 checks every round row's
clock, pointer, index, input and output against the reference's 56
intermediate states. P3 sweeps four round rows and both controller rows. P4
mutates real rows, half of them inside interaction-bound fields, and requires
each to fail the row algebra, leave the 55-row chain unbalanced, or change an
argument contribution.

**The fixed-modulus field chip.** FP1 runs the `PALLAS_FP_*` and `VESTA_FP_*`
syscalls through a real program that also compares each result in-guest. FP1b
is the boundary that matters here: the executor reduces its operands and
`FpOpChip` does not, so a noncanonical operand's honest row may have no
satisfying witness at all, the carry having outgrown its limbs. Which rows do
is reported per row rather than ruled, and a row that does satisfy the AIR must
carry the reduced result. FP2, FP3 and FP4 are the witness, sweep and mutation
layers for the same chip.

**The two executors.** Every Pasta syscall is served twice in the fork — the
minimal executor computes, the tracing executor records — and
`generate_records` runs both in sequence, so a disagreement never surfaces as a
wrong result, only as one side accepting a program the other refuses. D1 is a
table of malformed register arguments (misaligned pointers, addresses outside
the valid range, a nonzero second argument where the ABI requires zero, the
all-zero operand, a coordinate above the modulus); D2 draws random pointer
offsets. Both carry plain store-and-load controls, with no syscall in the
program, which attribute a divergence to the address rather than to the
precompile.

**The native Pickles verifier.** V1 mutates one typed field of a
`VerifiableProof` — statement field, deferred challenge, evaluation,
commitment, IPA response — with a random field element or a negated point,
drawing each of the 18 mutators once before any random draw. V2 flips one
random bit of the serialization; most flips do not deserialize, which is
counted apart. Both require rejection over all nine fixtures.

**The canonical residues.** A1 crosses nine boundary classes pairwise and adds
random pairs and triples, covering `add_assign`'s conditional subtraction, both
`pow7` schedules and all three three-term inner products, and requires
`from_limbs` to reject `p`, `p + 1` and `2^256 - 1`. On the host the products
delegate to arkworks, so the subject is the logic around them, not the
multiplication.

## Running them

The harness writes `result.json` into a directory that must not already exist;
`verifier-fuzz` writes one file. Seeds accept decimal or `0x`, are printed and
are recorded in the report, so a failure replays.

```bash
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 PASTA_FUZZ_SEED=0x5eed0301 PASTA_FUZZ_ITERS=32 cargo run --release --locked --manifest-path tools/pasta-proof/Cargo.toml --bin pasta-precompile-proof-6-10 -- --mode pasta-fuzz --output /tmp/pasta-fuzz-0301
```

```bash
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 cargo run --release --locked -p cycle-runner --bin verifier-fuzz -- --seed 0x5eed0304 --typed 32 --bytes 64 --output /tmp/verifier-fuzz-0304.json
```

```bash
cargo test -p pickles-verifier --test canonical_words_fuzz
```

A run fails, with the offending cell or program in the message, when:

- a sweep leaves a cell that neither a constraint, an argument nor a fixed
  table rejects, and that is not on a padding row;
- a mutation is accepted by everything, or a tampered result limb satisfies the
  AIR;
- an honest row does not satisfy its AIR, an honest shard has an open argument
  message, or an honest chain is unbalanced;
- a forged degenerate row, a re-labelled round row or a swapped pair of
  permutation outputs passes both the algebra and the chain;
- the two executors disagree on a program no control attributes to the address
  or to the operand value;
- a mutated proof is accepted in any field other than the one the verifier
  reads nowhere, or that expectation is never exercised.

CI runs the harness's own unit tests (`cargo test --bin
pasta-precompile-proof-6-10`), not the fuzz layers; wiring a seeded run and a
nightly sweep into CI is phase 0/0b's open item in the plan.

## What the recorded runs measured

Four `pasta-fuzz` runs (`0x5eed0301`, `0x5eed0302`, `0x5eed0303` at 32
iterations and `0x5eed0202` at 16) and one `verifier-fuzz` run (`0x5eed0304`):

| Layer | Volume, all runs | Outcome |
| --- | --- | --- |
| F1–F3 | 544 operations, 1,599 + 1,591 columns | no disagreement, zero surviving columns |
| P1–P3 | 68–69 inputs and 32 permutations per field, 9,206 cells | survivors only in interaction-bound fields |
| P4 | 448 mutations | 377 row algebra, 61 round chain, 10 memory argument, 0 unexplained |
| FP1/FP1b | 474 Pallas and 492 Vesta operations | executor matched the reference on every one |
| FP2–FP3 | 66–114 real rows, 1,224 cells per field and run | survivors all on the padding row |
| FP4 | 448 mutations | 447 row algebra, 1 fixed table, 0 unexplained |
| C1 | 6,396 add and 6,364 double cells per run | survivors all on the padding row |
| C2 | 8 forgeries per operation and run | all rejected; the `(0, 0)` row by the memory argument |
| D1/D2 | 68 cases, 32 offsets, 4 controls per run | 2 divergence classes, each attributed by a control |
| V1/V2 | 288 typed and 576 single-bit mutations | 0 accepted outside one ignored field, no panic |
| A1 | 593 pairs and 516 triples per field | all agreed with arkworks |

Evidence: [pasta-fuzz expansion summary](../benchmarks/pasta-fuzz-expansion-sp1-6.10.json),
[verifier mutation summary](../benchmarks/verifier-mutation-fuzz.json) and
[the Poseidon run](../benchmarks/pasta-poseidon-fuzz-sp1-6.10.json).

## What the layers found

- **A field the verifier does not read.** Every mutation the native verifier
  accepted was a mutation of `p_eval0_chunks`. It is a copy of
  `prev_evals.public_evals.zeta` that the host conversion derives from it, and
  nothing in the verifier, the guest, the attestation or the statement digest
  reads it. The public-input evaluation is still bound, through the copy that
  is used; this one is dead weight the guest pays to deserialize. Removing it
  is an ABI change, left as its own question.
- **A pointer below SP1's 2^16 data floor** splits the two executors: the
  minimal one completes, the tracing replay panics inverting zero. A plain
  store and load at the same address, with no syscall in the program,
  reproduces it, so it belongs to the address.
- **The all-zero operand** splits them on the double chips only, because the
  add chips' batched-inversion path populates the same columns without
  asserting, so the record is built and the row is merely unsatisfiable. Both
  classes are programs that execute but cannot be traced, hence cannot be
  proved; neither is a soundness question.
- **The double chip's free slope** at `(0, 0)`, which the row algebra accepts,
  changes the claimed input in memory when written over an honest trace, so the
  memory argument rejects it. The algebraic gap and the `2y != 0` precondition
  are unchanged; what is new is that this way of exploiting it does not reach a
  proof.

The fuzzing also found two defects in itself, both recorded in
[the corrections](optimizations/08-experiments-and-corrections.md): a
differential that compared the executor's words against an empty list, because
the field operations file their events under the add syscall code, and a typed
verifier mutator that could pass by never being drawn.

## What none of this covers

- **Coordinated multi-column forgeries.** Every sweep and mutation layer moves
  one cell. C2's whole-row forgeries are the known cases only.
- **A prover that rewrites both sides of an argument.** The channel shows that
  an otherwise honest shard cannot absorb the mutation; a prover free to
  rewrite the chips on the other side is outside the model. That is the
  full-proof variant of F4, still open.
- **Absence of a gap.** Fuzzing searches the input space; only the Lean
  soundness theorems in the plan can say a gap is not there. The planned
  layers F5 (Rust–Lean randomized evaluation) and F6 (the same shapes at
  secp256k1 and BN254) do not exist yet.
- **The in-circuit verifier and SP1 itself.** V1 and V2 test the native
  verifier only; the recursion verifier, the prover and the guest's MSM logic
  are out of scope everywhere here.
- **The multiplication in `CanonicalPasta`.** On the host it is arkworks, so
  A1 compares the schedules, accumulation and reduction around it.
- **User-mode page-protection columns and proofs.** The page-protection
  argument participates in the channel, but the user-mode variants' own
  survivor sets are phase 5 in the plan.
