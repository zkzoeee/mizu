# Poseidon, constants and scalar challenges

Status: retained exact Kimchi Poseidon implementation using existing constrained
field arithmetic. The current guest has no custom Poseidon permutation chip.

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
