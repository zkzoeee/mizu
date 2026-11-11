# Measurement, audits and development tooling

Status: retained CPU measurement and correctness tooling, with historical
accounting corrections explicitly recorded. A profile is diagnostic evidence;
it is not automatically the timing of an uninstrumented ELF.

## Work records

| Work | Historical rounds | Evidence rule / retained result |
| --- | --- | --- |
| Decompose shared MSM and field cost | [r13/r14/r17/r18](round-index.md#r18) | Separate execution from witness generation and confirm operation counts before choosing a target. |
| Profile GLV, carry geometry and actual selected windows | [r33–r41](round-index.md#r35), [r59/r60](round-index.md#r60) | Apply barrier/calibration corrections; do not infer live width choices from unreachable code. |
| Stage boundaries and frozen instruction attribution | [r42/r45/r48/r64/r80/r88/r94/r96/r104](round-index.md#r94) | Frozen-ELF instruction totals and separately instrumented diagnostic guests are different evidence classes. |
| Correct record accounting | [r53/r54/r71/r72](round-index.md#r54) | Generate dependencies once; count only included chips; distinguish stored main cells, official shape area and physical prover allocation. |
| Native/build loop and checked blob cache | [r61/r66/r67](round-index.md#r67) | Faster development and cache correctness, with no claimed guest-cycle gain. |
| Small-operation benchmark foundation | [r68](round-index.md#r68) | Compare independent arithmetic answers, cold startup and larger workloads before full-guest timing. |
| Completion/correctness audits | [r103/r113/r114/r118/r119](round-index.md#r118) | Preserve both retained and failed variants, source/config/ELF/runner identities, exact bytes and verification outcomes. |
| Matched versions and pipeline throughput | [r121–r126](round-index.md#r124) | Same host/options and repeated runs are required for version/wall-time attribution. CPU accounting and real producer-consumer pipelines can have different bottlenecks. |

The corrected r72 comparison used its own matched producer/options: r31
427,791,614 → current-at-r72 252,012,129 cycles; stored main cells
98,162,838,112 → 57,029,321,632. It is historical CPU record accounting, not
new proving-time evidence and not the current 0.7 guest's cell count.

## Implementation and reproducibility

- [Retained recipe](../../benchmarks/retained.json),
  [build/execution script](../../scripts/benchmark.py) and
  [cycle runner](../../crates/cycle-runner/src/main.rs).
- [Trace accounting](../../tools/pasta-proof/src/trace_accounting.rs),
  [instruction profile](../../tools/pasta-proof/src/instruction_profile.rs),
  [run manifest](../../tools/pasta-proof/src/run_manifest.rs), and
  [retention audit](../../tools/pasta-proof/src/retention_audit.rs).
- [Guest blob cache](../../crates/o1-verifier/blob_cache.rs) and its
  [tests](../../crates/pickles-verifier/tests/verifier_blob_cache.rs).

## Current comparable checkpoints

| Checkpoint | SP1 / proof-systems | ABI | Cycles |
| --- | --- | --- | ---: |
| Historical r111 | 6.1 / original historical source | 577-byte receipt | 142,692,781 |
| Historical r121/r122 runtime rebuild | 6.10 / original historical source | 577-byte receipt | 142,692,323 |
| Consolidated Mizu | 6.10 / optimized 0.4.1 | one-byte boolean | 142,660,047 |
| Current Mizu | 6.10 / optimized 0.7.0 | one-byte boolean | 142,650,295 |

The final two checkpoints are a matched comparison: −9,752 cycles (−0.006836%),
same fixture, serialized input, 40 settings, toolchains, output and arithmetic
calls. Receipt-to-boolean changes are not a pure optimization ablation. See
[the current measurement](../../benchmarks/core-proof-systems-0.7-sp1-6.10.json).

For every future record, retain the control and candidate's actual source/ELF,
compiler/builder, flags, fixture/input/output hashes, metric, acceptance/rejection
results and memory/options. Report failed/incomplete attempts as such. Do not
sum deltas measured against different controls or substitute shape estimates for
wall-clock proof time. Use [the record template](TEMPLATE.md).

## Retained controls and commit ownership

No additional setting from the 40-flag retained recipe is assigned to this area.
Optional harness/routing/GMP settings do not become defaults through this record.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.

## Local validation review

The [Miza validation review](miza-validation-review.md) records stronger comparator,
guest-matrix and proof-handoff patterns, existing coverage overlaps, 13 selected
Python gates actually run, and the distinction between compiled/simulated proof
matrices and completed cryptographic verification. No dependency code was ported.

## mizu-cached-dev-loop: reusable development and promotion gates

- **Status:** retained tooling; the full CPU promotion gate and its checked
  repeat passed. [Workflow evidence](../../benchmarks/dev-loop-validation.json)
  separates the original complete matrix from later evidence-storage rechecks.
- **Owner:** root `scripts/dev.py`, `scripts/devloop`, `scripts/tests`,
  `crates/cycle-runner`, `crates/poseidon-bench`, Make and CI integration.
- **Mechanism:** source/flag/tool-addressed guest builds; frozen runners,
  source archives and checked execution reuse; recipe-matched native checks;
  separate targeted, microbenchmark, paired-verifier and promotion stages.
- **Predecessors:** r66 native-test/build profile, r67 authenticated blob cache,
  r68 standalone Poseidon measurement and the original experiment-cache driver.
  Miza's comparator/tests and source-matched native/guest regression patterns
  were adapted to the current boolean/statement-digest guest contracts.
- **Implementation topics:** 29818e2 restores the microguest/oracle; a2a0bd2
  adds six deterministic runner mutations; e249a3b adds cache/comparison/native
  and promotion commands; cb34107 freezes per-run input files, tightens native
  receipt completeness and fixes reused-control labeling; d3dc445 scopes guest
  cache guards to the root consumer instead of the standalone tools workspace.
  These topic commits do not modify dependency forks.
- **Control/candidate:** Poseidon recipes compare existing ordinary/canonical
  paths, 2,053,847 → 1,565,472 cycles, independently checked on both fields.
  This validates the screening tool, not a newly added whole-verifier gain.
  A checked repeat took 2.293 seconds on this host and reused both original
  observations; it is not an independent measurement. The full verifier pair
  reproduced 142,638,298 → 142,553,240 cycles and an accumulator rejection.
- **Correctness:** strict comparator tests cover compatibility fields, numeric
  types, duplicate JSON keys and reused evidence. Cache tests corrupt actual
  artifacts and archives. Native checks reject zero selected tests and match
  recipe settings. Promotion checks require six changed witnesses, a distinct
  wrong-key ELF, positive producer fixtures, output/exit gates and matching
  bounded trace replay. All nine fixtures, seven rejection cases, 215 native
  tests, no-std and replay passed, including a checked repeat of the gate.
  Hardening adds tests for frozen-input corruption, incomplete native receipts,
  interrupted attempts, explicit no-proof flags and immutable control reuse.
  All 31 tooling tests and a frozen-operation input-storage/reuse check passed
  after hardening. Concurrent harness/runtime edits interrupted fresh build
  comparisons and remain outside this record's completed validation. A redundant
  native repeat was stopped; its partial logs do not count as another passed suite.
  Raw logs and failed attempts stay outside Git.
- **Validation scope:** CPU builds, native tests, guest execution and trace
  estimates; no SP1 proof generation, proof verification/reload or CUDA work.
  Runtime changes still require the focused Pasta AIR/record/proof gates.
- **Upstream sync:** replay the root tooling/microguest/runner topic commits;
  refresh the pinned builder and recipes deliberately. Do not import historical
  SP1 6.1 manifests, application code or Miza's unproved runtime into this loop.
  The [development guide](../development-loop.md) documents commands and scope.

## mizu-pasta-fuzz: differential fuzzing of the Pasta precompiles

- **Status:** validation tooling, retained. No performance claim.
- **Area and owner:** `tools/pasta-proof/src/main.rs`, mode `pasta-fuzz`; tests
  in `batch_inversion_tests`.
- **Mechanism:** three layers against the harness's independent integer
  reference (`ref_add`, `ref_double`): executor curve operations against the
  generic formulas, including degenerate and noncanonical inputs that must
  behave identically on both paths, panics included; random operands through a
  real program, checking every executor result, every witness row's `x3` and
  `y3`, and the AIR on honest rows; and a sweep that perturbs every column of
  an honest row and evaluates the chip AIR with `debug_constraints`.
- **Selection:** `--mode pasta-fuzz --output DIR`, seeded by `PASTA_FUZZ_SEED`
  and sized by `PASTA_FUZZ_ITERS`. The seed is printed and recorded.
- **Result:** seeds `0x5eed0001` (32 iterations), `0x5eed0002` and
  `0x5eed0003` (64) all passed: 544 operations, zero surviving columns in all
  four chips, degenerate-input parity on both curves.
- **Limits:** algebraic constraints only; interactions are not evaluated, and a
  single-column sweep cannot find coordinated multi-column forgeries. The plan
  for lookup-aware mutation and Lean proofs is in
  [the verification plan](../pasta-air-verification-plan.md). The first of
  those limits was lifted the same day: the layers below evaluate the memory,
  page-protection and syscall arguments and the fixed tables' message sets, so
  a surviving column is no longer read as an unconstrained one. The multi-row
  and multi-column limits stand.

The same mode also extends `r5-cycles --time-witness`: it now times Pasta
dependency and trace generation separately, and with `PASTA_WITNESS_DIGEST=1`
it digests every Pasta chip trace and its sorted byte lookups over the whole
workload. Equal digests across two builds mean the witness is bit-identical
for every real addition, not just for the one-event `witness-digest` fixtures.

## mizu-fuzz-expansion: five more fuzz surfaces and the argument channel

- **Status:** validation tooling, retained. No performance claim, no change to
  any retained setting.
- **Area and owner:** `tools/pasta-proof/src/main.rs` (mode `pasta-fuzz`: new
  layers FP1–FP4, C1–C2, D1–D2, and the argument channel used by FP3, FP4, C1
  and Poseidon P4), `crates/cycle-runner/src/bin/verifier-fuzz.rs` (new
  binary), `crates/pickles-verifier/tests/canonical_words_fuzz.rs` (new test).
  No fork change, so the pins are unmoved.
- **Implementation identity:** root commits `568c696` (harness) and `17a5628`
  (native harnesses) on `codex/zakura-ragu`, over the fork pins of
  `mizu-pasta-poseidon-precompile` (SP1 `a418319a0`, proof-systems
  `ddb617200`), which this work does not touch. The measurements come from
  those commits' source.
- **Predecessors/corrections:** extends
  [`mizu-pasta-fuzz`](#mizu-pasta-fuzz-differential-fuzzing-of-the-pasta-precompiles)
  (curve layers F1–F3) and the Poseidon layers P1–P4 recorded under
  [Poseidon](04-poseidon.md#mizu-pasta-poseidon-precompile-the-kimchi-permutation-as-one-syscall).
  It supersedes two stated limits: P4's "memory and syscall arguments are not
  evaluated", and F3's reading of a surviving column as unconstrained.
- **Mechanism:** the channel and five surfaces, in the order they were built.
  1. *The argument channel.* `debug_constraints` evaluates one chip's row
     algebra and nothing else, so a column bound by a lookup argument looks
     unconstrained to a column sweep. The harness now compares the chip's own
     contribution to the memory, page-protection and syscall arguments before
     and after a mutation. Those balance over every chip of a shard, so a
     changed contribution cannot be absorbed by an otherwise honest shard: it
     is a rejection, with no proof generated. The honest balance is checked per
     layer over every participating chip rather than assumed. Byte lookups are
     treated differently, because the byte and range tables' multiplicity
     columns are prover-supplied: what binds there is membership of the
     message set, which comes from the tables' preprocessed traces. The
     harness enumerates that set once per run and asks whether a mutated row
     requests a message no table holds.
  2. *The fixed-modulus field chip* (FP1–FP4): the executor against a BigUint
     reference on random canonical, edge and noncanonical operands; every real
     row's result limbs; the column sweep across four rows instead of the
     first; and random single-cell mutations.
  3. *The curve chips beyond the first row* (C1–C2): the sweep across four
     rows of a six-call trace, and randomized forgeries of the two degenerate
     cases the fixed suite pins at one point each.
  4. *The two executors* (D1–D2): every Pasta syscall is served twice in the
     fork, by the minimal executor which computes and the tracing executor
     which records, and `generate_records` runs both in sequence. A
     disagreement therefore never shows up as a wrong result, only as one side
     accepting a program the other refuses. Malformed register arguments and
     random pointer offsets are put to both, with controls that attribute a
     divergence to the address or to the operand value rather than to the
     syscall.
  5. *The native Pickles verifier* (V1–V2): one randomly chosen typed field of
     a `VerifiableProof`, or one random bit of its serialization, mutated and
     required to be rejected, over all nine fixtures.
  6. *The canonical Pasta residues* (A1): against arkworks on boundary classes
     and random operands.
- **Selection:** `--mode pasta-fuzz --output DIR` with `PASTA_FUZZ_SEED` and
  `PASTA_FUZZ_ITERS` as before; `verifier-fuzz --seed --typed --bytes
  --fixtures --output`; `cargo test -p pickles-verifier --test
  canonical_words_fuzz`. Nothing here runs in any proving path.
- **Result:** four `pasta-fuzz` runs — `0x5eed0301`, `0x5eed0302` and
  `0x5eed0303` at 32 iterations, and `0x5eed0202` at 16, the seed whose
  low-byte cell prompted the fixed-table check and the only one of the four
  that exercises it — at 217 to 236 seconds a run; and one verifier-fuzz run
  (seed `0x5eed0304`, 32 typed and 64 single-bit mutations per fixture, 413
  seconds). No cell was left that neither a constraint, an argument nor a
  fixed table rejects, no mutated proof was accepted outside the one field the
  verifier does not read, and no executor divergence went unattributed. The
  fixed tables hold 459,007 distinct byte-lookup messages, enumerated once per
  run from their preprocessed traces.

| Layer | Volume, all runs | Outcome |
| --- | --- | --- |
| FP1/FP1b | 474 Pallas and 492 Vesta field operations, 66 and 84 of them noncanonical | the executor matched the reference on every one; of the noncanonical rows, 44 and 56 had a satisfying witness and every one of those carried the reduced result |
| FP2 | 66 or 114 real rows per field and run | row results matched the executor and the reference; honest rows satisfied the AIR; no padding row was real |
| FP3 | 1,224 cells per field and run | 112 survived the row algebra each time, all of them on the padding row |
| FP4 | 448 mutations | 447 rejected by the row algebra, 1 by a fixed table, 0 unexplained |
| C1 | 6,396 (add) and 6,364 (double) cells per run | 210 (add) and 140 (double) survived the row algebra, all on the padding row |
| C2 | 8 forgeries per operation and run | every forgery rejected by the row algebra except the double chip's `(0, 0)` row, which the memory argument rejected |
| D1/D2 | 68 table cases, 32 random offsets and 4 plain-store controls per run | 8 divergences from the address floor and 2 to 4 from a malformed operand value, each attributed by a control; 0 unexplained |
| P4 | 448 mutations | 377 rejected by the row algebra, 61 by the round chain, 10 by the memory argument, 0 unexplained |
| V1/V2 | 9 fixtures, 288 typed and 576 single-bit mutations | 271 typed and 375 single-bit mutations rejected, 197 flips did not deserialize, 18 accepted in the one ignored field, 0 accepted anywhere else, no verifier panic |
| A1 | 593 operand pairs and 516 triples per field | all agreed with arkworks |

- **Correctness evidence and what the layers found:**
  - *The address floor.* Pointing any Pasta syscall below SP1's 2^16 data
    floor makes the minimal executor complete and the tracing replay panic in
    `p3_field`'s inverse. The plain store-and-load control reproduces it with
    no syscall at all, so it is a property of the address, not of these
    precompiles, and the harness attributes it there. Programs that execute
    but cannot be traced cannot be proved; nothing about soundness changes.
  - *Malformed operand values.* On the two double chips the executors also
    part company over the all-zero encoding: the minimal executor's fixed-limb
    path returns zero for the inverse of zero and completes, while record
    generation panics in the witness generator's division. The add chips do
    not, because their batched-inversion path populates the same columns
    without asserting, so the record is built and the row is simply
    unsatisfiable. Random pointer offsets found this class twice before it had
    cases of its own, once through untouched memory and once through a rotated
    coordinate that underflows a witness-generator subtraction. The operand
    pages are now filled with repeated valid words, so an offset of whole
    operands reads a well-formed value and tests the address alone, while an
    offset of whole words rotates the operand and is attributed to the value.
  - *Noncanonical field operands.* The executor reduces its operands and
    `FpOpChip` does not, so the two disagree about what a noncanonical operand
    means. The executor's result is always the reduced reference. Whether the
    honest row for such an operand satisfies the AIR depends on the operands
    and the operation, because the carry the witness generator computes can
    outgrow its limbs: in the recorded runs most subtractions had no satisfying
    witness and most additions and multiplications had one. Every row that does
    satisfy the AIR is required to carry the reduced result, so the class is a
    liveness boundary and not a second meaning for the same memory words. It is
    the same boundary as the curve chips' noncanonical inputs.
  - *The low-byte columns.* `U16toU8Operation` keeps only the low byte of each
    16-bit memory limb as a column and derives the high byte as
    `(limb - low) / 256`, so a single-cell mutation of a low byte moves both
    bytes and leaves the limb, and the memory argument, intact. Nothing but
    the byte range check rejects it, and it always does: one of the two bytes
    must leave `[0, 256)`. The first run of FP4 reported such a cell as
    unexplained, which is what prompted the fixed-table membership check; it
    is now a counted class.
  - *The double chip's free slope.* Written over an honest trace, the `(0, 0)`
    forgery the row algebra accepts changes the claimed input in memory, so
    the memory argument rejects it. The chip's algebraic gap is unchanged, and
    so is the conclusion that the guest ABI must discharge `2y != 0`; what is
    new is that this particular way of exploiting it does not reach a proof.
  - *A field the verifier does not read.* Every mutation the native verifier
    accepted was a mutation of `p_eval0_chunks`, on eight of the nine
    fixtures. It is a copy of `prev_evals.public_evals.zeta` that the host
    conversion derives from it, and nothing in the verifier, the guest, the
    attestation or the statement digest reads it: the proof's public-input
    evaluation is bound through the copy that is used, and this one is dead
    weight in the guest's input, which the guest still pays to deserialize.
    The harness now expects those acceptances, counts them apart, requires the
    expectation to be exercised and fails if any other field turns out to be
    ignored. Removing the field would be an ABI change and is left as a
    separate question.
  - *A gap in the harness itself.* The field operations file their events
    under the add syscall code, so the differential's first version compared
    the minimal executor's words against an empty list and passed vacuously.
    The baseline assertion added for that reason now requires the well-formed
    call to produce one event and the same words on both sides. The typed
    verifier mutators are likewise drawn once each before the random draws
    begin, so a mutator that is never drawn cannot pass by not running.
- **Validation scope:** host execution, row and AIR evaluation, argument
  multisets, fixed-table membership, native Pickles verification, native field
  arithmetic. No proof is generated anywhere in this record, no GPU, no
  recursion.
- **Decision and limits:** retained as tooling. The argument channel shows
  that an otherwise honest shard cannot absorb the mutation; a prover that
  also rewrites the chips on the other side of an argument is outside what it
  measures, and the full-proof variant of F4 stays open. The native verifier
  layers say nothing about the in-circuit verifier or about SP1. On the host,
  `CanonicalPasta`'s products are arkworks, so A1 covers the schedules,
  accumulation and reduction around them, not the multiplication. Single-cell
  sweeps still cannot find coordinated multi-column forgeries, which is what
  the Lean work in [the verification plan](../pasta-air-verification-plan.md)
  is for.
- **Source provenance:** [pasta-fuzz expansion
  summary](../../benchmarks/pasta-fuzz-expansion-sp1-6.10.json) and [verifier
  mutation summary](../../benchmarks/verifier-mutation-fuzz.json), both with
  the raw report hashes. [The fuzzing reference](../fuzzing.md) collects what
  every layer covers, how to run it and what it does not cover.
- **Upstream sync:** none. No fork file changed, so there is nothing to replay.

## mizu-build-cache-volume: the volume carries the build, and a fat binary is free at runtime

- **Status:** validation-only, retained. The volume and the multi-architecture
  build are now the recommended routine; three driver defaults changed.
- **Area and owner:** `scripts/gpu/{push-and-launch,pod-run,watchdog,collect-results}.sh`,
  network volume `zvx1pq06vk` ("mizu-build-cache", 10 GB STANDARD, EU-SE-1).
- **Implementation identity:** root `7c10243` plus the driver fixes below.
- **Predecessors:** the cache machinery landed unexercised with
  [the drivers](#mizu-fuzz-expansion-five-more-fuzz-surfaces-and-the-argument-channel)'
  sibling work; this is its first real test, on the manifest arm of
  [mizu-gpu-core-proof-a40](06-sp1-runtime-and-proving.md#mizu-gpu-core-proof-a40-the-first-real-gpu-proof-from-this-repository).
- **Mechanism:** two pods, one after the other, same volume mounted at
  `/workspace`. The first built `CUDA_ARCHS=86,89,120` and populated
  `/workspace/mizu-cache`; the second, a separate pod, found the same key and
  compiled nothing.

| | run 1, populate | run 2, hit |
| --- | --- | --- |
| Pod | `7mvcnoafazzpet` | `lxvxl82gr73er4` |
| `cache_hit` | 0 | **1** |
| Key | `3865a02c…` | **`3865a02c…` (same)** |
| Build | **12 min 31 s** | **none** |
| In-pod work | ~15 min | **3 min 3 s** |
| Pod life / cost | 20.5 min / ~$0.20 | 7.0 min / ~$0.07 |
| Proof wall | 136.14 s | 137.98 s |
| Shards, bytes | 82, 132,029,932 | 82, 132,029,932 |
| Key, digest | `eed89d80…`, `4bf5122f…` | identical |
| Accepted, reloaded from disk | yes | yes |

- **Result:** the key matched across two separate pods and the second run did
  everything — prerequisites, cache check, pre-flight, proof, from-disk
  verification — in **3 minutes 3 seconds** with no compilation. The cache is
  5.5 GB of the 10 GB volume, `cargo-home` being 4.8 GB of that.
- **The fat binary costs nothing at runtime.** `86,89,120` covers `sm_86`
  (A40, A6000, 3090), `sm_89` (4090, L40S) and `sm_120` (5090, PRO 4500/6000)
  from one artefact, which is the answer to the stock lottery that cost nine
  refused creates in one sitting. It costs **2.07x the binary size**
  (120,555,456 against 58,552,272 bytes) and a one-time build of 12.5 minutes
  against ~7 for a single architecture. Proof wall moved +1.4%, inside the
  0.8–0.9% spread measured for this host class.
- **Correctness evidence:** both runs accepted by the complete verifier and
  both reloaded from disk in a fresh process under a key derived from the
  pinned ELF, with identical shard counts, proof byte counts, verification key
  and committed digest. Only the proof hashes differ, because FRI draws
  randomness.
- **Validation scope:** Linux/CUDA build, real CUDA core proofs, on-pod
  reload verification. No off-pod verification: proof bytes stayed on the pod.
- **Decision and limits:** retained as the routine. **The cache key is exact,
  not subsumptive** — a run that lets the driver auto-detect `CUDA_ARCHS` from
  the card computes `86` and misses an `86,89,120` cache, so a fat cache must
  be used with the same value. A superset ought to count as a hit and does
  not. `df` on the mount reports the whole MooseFS pool rather than the quota,
  so only `du` on the cache is informative. One hit and one miss here; nothing
  measured about a key that moves for a source edit. Run 1's leftovers from
  the old defaults (~100 MB) are still on the volume.
- **Driver defaults this exposed and changed:** `ROOT` and `RESULTS` pointed
  into `/workspace`, i.e. onto the volume, so an unpacked tree and a results
  directory survived the pod and would have blocked the next run — both now
  default to container disk, leaving only the cache on the volume;
  `push-and-launch.sh` now forwards `RESULTS`; the watchdog takes `LOG_DIR`;
  and `collect-results.sh`, which hardcoded `/workspace/results` in five
  places and staged its archives on the volume, now takes `POD_RESULTS` and
  stages on container disk.
- **Source provenance:**
  [gpu-build-cache-validation.json](../../benchmarks/gpu-build-cache-validation.json),
  receipts in `benchmarks/gpu/runs/20261111T0628Z-fatbuild-populate` and
  `…/20261111T0649Z-cachehit`. ~$0.27 for both runs; pods terminated.
- **Upstream sync:** none.

