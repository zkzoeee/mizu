# Poseidon, constants and scalar challenges

Status: retained exact Kimchi Poseidon implementation using existing constrained
field arithmetic. An opt-in whole-permutation chip pair exists in the SP1 fork
(`mizu-pasta-poseidon-precompile`, below); the retained recipe does not use it.

## Work records

| Work | Historical rounds | Recorded outcome |
| --- | --- | --- |
| Compile-time Kimchi constants | [r57](round-index.md#r57) | 270,998,816 → 268,092,468. Profiling found decimal parameter initialization rather than the originally suspected oracle arithmetic. |
| Legacy parameter coverage | [r61/r62](round-index.md#r62) | Compatibility tests and development-loop work, with zero retained cycle gain claimed for these rounds. |
| Canonical-state 55-round permutation | [r69](round-index.md#r69) | 258,722,659 → 254,032,632; UInt256 calls 1,212,429 → 1,025,226. Restrict dispatch to exact supported fields, round configuration and immutable parameter statics. |
| Prepared RHS/modulus operands | [r70](round-index.md#r70) | 254,032,632 → 252,012,129, with the same 1,025,226 UInt256 calls. Reuse packed constants and multiplication operands. |
| Integer scalar-challenge conversion | [r74](round-index.md#r74) | 252,012,129 → 251,710,934; retain the same challenge mapping. |

All cycle figures are historical full receipt-ABI executions. The r69 small
one-per-field benchmark regressed despite the full guest win; r70 amortized that
startup cost differently. Workload size and first-use preparation belong in the
record, not just the best headline.

## Implementation and correctness

- [Canonical permutation](../../vendor/proof-systems/poseidon/src/canonical.rs),
  [dispatch/permutation](../../vendor/proof-systems/poseidon/src/permutation.rs), and
  [scalar challenge conversion](../../vendor/proof-systems/poseidon/src/sponge.rs).
- Compile-time parameters under `vendor/proof-systems/poseidon/src/pasta/`:
  `fp_kimchi.rs`, `fq_kimchi.rs`, `fp_legacy.rs`, `fq_legacy.rs`.
- Tests: `canonical_poseidon`, `sponge_constants`, `legacy_sponge_constants`
  and `scalar_challenge`; historical independent integer-oracle checks are in r69/r70.

The optimized path preserves the exact constants, round count and transcript
outputs. Unsupported/custom parameter tables use the generic path. On the 0.7
base, prepared-table initialization uses upstream's no-std-compatible `LazyLock`.

## Prototype and upgrade boundary

[r110](round-index.md#r110) built a standalone exact-round AIR and integer-bound
checks. It is a prototype in
[the Pasta harness](../../tools/pasta-proof/src/poseidon_round.rs), with no
production syscall, authenticated memory binding or complete permutation linkage.
Its modeled multiplication savings are not a measured guest-cycle or proof-time
win. Keep that experiment separate from the retained r69/r70 software path.
After an upstream sponge change, rerun constants/transcript and independent
arithmetic comparisons before timing the full fixture.

## Retained controls and commit ownership

| Setting | Current value |
| --- | --- |
| `ZEKO_CANONICAL_POSEIDON` | `1` |
| `ZEKO_POSEIDON_PACKED_RHS` | `1` |
| `ZEKO_INTEGER_SCALAR_CHALLENGE` | `1` |

These controls can compose or select fallback paths. Their grouping assigns a
primary documentation area, not independent additive performance credit. Exact
source references are in [the file appendix](../file-changes.md).

Pinned topic commits (in their respective repositories):

- proof-systems `3d579636d` — Add canonical Poseidon and retained sponge fast paths.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.

## mizu-poseidon-deferred-mds: one reduction per MDS row

- **Status:** opt-in measured candidate, proof-systems fork topic `8d691e991`;
  not in the retained recipe.
- **Area and owner:** proof-systems fork,
  `curves/src/pasta/canonical_words.rs` (`sum3_prepared_products_deferred`) and
  `poseidon/src/canonical.rs`; recipes `benchmarks/dev/deferred-mds.json` and
  `poseidon-deferred-mds.json`.
- **Predecessors:** r69/r70 prepared canonical Poseidon; r110 modeled the same
  75,570-call reduction with a dedicated AIR. Idea adapted from Zakura's
  deferred Montgomery reduction for inner products.
- **Mechanism:** each MDS row's three products accumulate unreduced as a
  512-bit value through `UINT256_MUL_CARRY`, which SP1 keeps in CPU shards by
  default, and reduce once with a single `UINT256_MUL` by `2^256 mod p`.
  Bounds are in the function's comment.
- **Selection:** `MIZU_POSEIDON_DEFERRED_MDS=1` at guest build time. Default off.
- **Result:**

| Measurement | Control | Candidate | Change |
| --- | ---: | ---: | ---: |
| Poseidon operation guest cycles (32 permutations) | 1,565,472 | 2,076,518 | +32.6% |
| Full guest cycles | 142,638,298 | 146,204,861 | +2.50% |
| Full guest `UINT256_MUL` calls | 533,878 | 458,308 | −75,570 |
| Main cells, default routing | 43,922,703,280 | 43,395,469,248 | −1.20% |
| Global interaction events | 136,299,445 | 133,568,754 | −2.00% |

- **Correctness evidence:** six native tests, including every Poseidon round on
  both fields against Mina's permutation and an edge-value test at `p − 1`;
  no-std; the operation guest's outputs against the independent integer
  oracle; the full guest accepts the mainnet proof and rejects a corrupted
  accumulator challenge (guest exit 1, no output).
- **Decision and limits:** the first change in this record that trades cycles
  for trace volume deliberately. Keep opt-in. Stored cells are not proving
  time. Under retain-UInt routing the removed calls already deduplicate in CPU
  shards, so the saving should shrink; that is unmeasured. Promotion needs a
  real proof comparison on the production routing.

## mizu-pasta-poseidon-precompile: the kimchi permutation as one syscall

- **Status:** opt-in measured candidate; not in the retained recipe. CPU-proved
  on a synthetic program; no GPU proof and no recursion VK map for the new
  clusters.
- **Area and owner:** SP1 fork: `crates/curves/src/weierstrass/pasta_poseidon.rs`
  (host permutation, field types, constants generated from the proof-systems
  export in `pasta_poseidon_constants.rs`), executor syscalls
  `PALLAS_POSEIDON` (`0x0001017A`) and `VESTA_POSEIDON` (`0x0001017B`) with
  events, minimal and tracing handlers, AIR ids 134 to 139, and the chips in
  `crates/core/machine/src/syscall/precompiles/pasta_poseidon/`. Proof-systems
  fork: `curves/src/pasta/canonical_words.rs` (`poseidon_kimchi_permute`) and
  `poseidon/src/canonical.rs` (`run_packed`). Root: harness mode
  `pasta-poseidon-check`, fixture
  `tools/pasta-proof/fixtures/poseidon-kimchi-parameters.json` (the r110
  parameter and oracle export, SHA-256
  `0eb7b577e205c32cb96cfff91c76275e37f1fab7912c592c346b380f938e41a6`), recipe
  `benchmarks/dev/pasta-poseidon.json`.
- **Implementation identity:** SP1 fork `a418319a0`, proof-systems
  `ddb617200`, algebra `6f81d7e13` (unchanged); root `codex/zakura-ragu`.
- **Predecessors:** r69/r70 canonical permutation (the software path this
  replaces inside the sponge); r110 modeled a dedicated Poseidon AIR without
  building it; `mizu-pasta-fp-precompile` is the control, since its syscalls are
  what each round used.
- **Mechanism:** one syscall per permutation on the twelve-word state replaces
  55 rounds of guest arithmetic: with the Pasta Fp precompile on, a round was 21
  fixed-modulus multiply syscalls plus additions and the surrounding loads,
  stores and loop control. The controller chip receives the syscall, reads the
  state at `clk`, writes the canonical result at `clk + 1` and anchors a 55-row
  chain of round rows; each round row proves `x^7` for the three lanes and the
  MDS rows with the selected round constant.
- **Selection:** `MIZU_POSEIDON_PRECOMPILE=1` at guest build time, on top of
  `MIZU_PASTA_FP_PRECOMPILE=1`; `run_packed` takes the syscall only on the
  zkvm target and only after `try_permutation` has authenticated the parameter
  table as the exact immutable kimchi statics, which are the chips' constants.
  Default off.
- **Control/candidate:** `benchmarks/dev/pasta-fp.json` against
  `benchmarks/dev/pasta-poseidon.json`, retained mainnet fixture, boolean ABI,
  output `01`, default routing, `scripts/dev.py bench` with the dev-cache
  runner; trace accounting on both ELFs with the same proof and receipt inputs
  (`649da7f4…`, `6e340b9c…`). Control ELF `49e23ca2…` (same cycles as the
  pasta-fp record's ELF; rebuilt because the proof-systems sources changed),
  candidate ELF `1f27156b…`.
- **Result (retained mainnet guest, default routing):**

| Measurement | Control | Candidate | Change |
| --- | ---: | ---: | ---: |
| Guest cycles | 139,610,314 | 130,367,324 | −6.62% |
| `pickles-wrap-kimchi` stage cycles | 53,800,406 | 48,960,261 | −9.00% |
| `pickles-wrap-prepare` stage cycles | 5,146,693 | 2,642,383 | −48.66% |
| `PALLAS_FP_*` / `VESTA_FP_*` calls | 400,111 / 133,767 | 206,071 / 63,312 | −194,040 / −70,455 |
| `PALLAS_POSEIDON` / `VESTA_POSEIDON` calls | 0 / 0 | 168 / 61 | 229 permutations |
| Main cells | 42,414,408,432 | 40,201,470,720 | −5.22% |
| Global interaction events | 130,836,329 | 123,706,642 | −5.45% |
| Preprocessed cells | 687,016,960 | 647,008,256 | −5.82% |
| Records | 145 | 137 | −8 |

  The new chips store 12,608 round rows (9,248 Pallas, 3,360 Vesta, padded
  from 229 × 55) and 256 controller rows: 25.95 M main cells, 0.065% of the
  candidate's total. Registered costs equal the widths: 2,048 (round), 507
  (controller) and 555 (user-mode controller). The accumulator stage is
  unchanged, as expected: its Poseidon work is in the other field's sponge
  only through the wrap stages measured above.
- **Correctness evidence:** `pasta-poseidon-check` (seed `0x0905e1d0`, 8
  permutations per field, the 4 oracle cases first): the fork's MDS and round
  constants equal the proof-systems export; the host permutation matches the
  independent integer oracle; executor outputs match a reference evaluated
  from the export's constants; every round row of every event carries its
  event's clock, pointer and index, the reference's state before the round as
  input and after it as output; honest rows of both chips satisfy both AIRs; a
  tampered MDS limb, power limb and second round flag are each rejected; the
  column sweep leaves only interaction-bound fields (round: `clk`, pointer;
  controller: `clk`, pointer, memory access and final-state bytes); the chain
  balances for the honest traces (448 messages, 0 open) and a controller
  claiming a different output leaves exactly two messages open; a 4-shard CPU
  core proof passes the complete verifier against the committed digest. The
  fork's `test_interaction_counts` passes with the borrowed kind. The full
  guest accepts the mainnet proof and rejects a corrupted accumulator; trace
  accounting accepts both complete executions. The harness found one bug
  before it reached a proof: the round trace generator sliced its buffer by
  rows instead of rows × width, so no event row was populated; the chain
  balance check caught it, and a per-row witness check now guards it.
  Differential fuzzing (`pasta-fuzz` layers P1 to P4, three distinct seeds,
  32 iterations each) passed: host permutation against two references on
  random, noncanonical and edge inputs; 1,760 round rows per field and seed
  against the reference; column sweeps on four round rows and both controller
  rows; 384 single-cell mutations, each rejected by the algebra or the chain
  or inside the memory-only field; re-labelled rounds and swapped outputs
  rejected. Details in the
  [verification plan](../pasta-air-verification-plan.md) and its
  [summary](../../benchmarks/pasta-poseidon-fuzz-sp1-6.10.json). The
  memory-only class was deferred in those runs and is checked in the later
  ones: P4 now requires such a mutation to change the chip's own contribution
  to the memory argument, which the rest of an honest shard cannot absorb. See
  [measurement and tooling](07-measurement-and-tooling.md#mizu-fuzz-expansion-five-more-fuzz-surfaces-and-the-argument-channel).
- **Validation scope:** compilation, CPU execution, row and AIR tests, lookup
  balance, one CPU core proof with complete verification. No fresh GPU proof,
  no recursion, no Linux/CUDA.
- **Decision and limits:** keep opt-in until a real proof comparison on the
  production routing exists; stored cells are not proving time. Under
  retain-UInt routing the removed `FP_*` calls already deduplicated in CPU
  shards, so the cell saving there is unmeasured. Not measured together with
  the deferred MDS flag, which the syscall makes moot inside the sponge.
  Evidence: [summary](../../benchmarks/pasta-poseidon-sp1-6.10.json).
- **Upstream sync:** two fork commits to replay (SP1 curves, executor and
  machine as one topic; proof-systems guest branch). SP1 upstream formats
  with an older rustfmt than the stable toolchain; the topic keeps upstream's
  formatting in touched files and formats only new files.

### Design and soundness notes

- **Shape.** SP1 Hypercube has no next-row transition, so the 55 rounds are
  55 rows of a round chip linked by chip-local lookup messages, with a
  controller chip that receives the syscall, reads and writes the 96-byte
  state, sends the round-0 message and receives the round-55 message. The
  same split SP1 uses for Keccak and SHA.
- **Borrowed message kind.** The harness consumes `sp1-hypercube` from
  crates.io, so the fork cannot add an `InteractionKind`. The chain uses
  `InteractionKind::Keccak` (106 values) with a tag at index 6 that no Keccak
  message can produce: Keccak messages carry one byte per value there, the
  tags are `0x0050414C` and `0x00564553`, both above `2^16`, and the two
  fields use different tags. A Poseidon message therefore never cancels a
  Keccak message or a message of the other field. The message is
  `(clk_high, clk_low, state_addr[3], round index, tag, 96 state bytes)`,
  zero-padded to 106.
- **What the row algebra binds.** Each round row proves `x^7` by four field
  multiplications and the MDS rows by a four-term inner product
  `M[i][0..3] . x7 + rc[r][i] . 1`, with the round constant selected by the
  one-hot step flags (`sum flags = is_real`, `index = sum r . flag_r`). The
  four-term product needs witness offset `2^15`: the bound is about 24,481,
  above the `2^14` offset the two-term `FieldOpCols` use, which is why the
  chips instantiate `PallasPoseidonField` / `VestaPoseidonField` rather than
  the curve base-field types. Inputs are byte-range-checked; outputs are
  bound by the next row's input check and, at round 55, by the controller's
  `FieldLtCols` canonical check before the memory write.
- **Caveats carried.** Round rows do not constrain `clk` and the state
  pointer algebraically; the chain and the controller's syscall and memory
  interactions do (the column sweep lists exactly those fields). Outputs of
  rounds 1 to 54 are reduced residues by the limb identity but not
  canonical: the inner product's result limbs are range-checked bytes, and
  the final `FieldLtCols` makes only the written state canonical, which is
  what the guest reads. The chain is a lookup argument, so its soundness is
  SP1's LogUp-GKR soundness; the harness checks balance on the honest trace
  and imbalance on a forged output, not the argument itself.

