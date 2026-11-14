# IPA, challenge coefficients and SRS storage

Status: retained optimized IPA and trusted embedded SRS paths, with ordinary
runtime/owned paths preserved. Fixed data is built from authenticated setup;
proof input cannot supply a callback, a replacement SRS or trusted tables.

## Work records

| Work | Historical rounds | Recorded outcome |
| --- | --- | --- |
| One IPA MSM instead of two parallel halves | [r46](round-index.md#r46) | 307,478,839 → 300,441,208; changes the group-operation schedule, not the verification equation. |
| Direct coefficient accumulation, preallocation and active SRS prefix | [r47](round-index.md#r47) | 300,441,208 → 287,349,624. The rejected filter-before-conversion variant instead copied new point buffers and regressed. |
| Public-input Lagrange prefix | [r49/r52](round-index.md#r52) | r49's prefix win was followed by a contract correction: prefix and full-domain caches must be distinct. The corrected checkpoint is 278,339,786. |
| In-place b-polynomial expansion and direct first-proof coefficients | [r75](round-index.md#r75), [r79](round-index.md#r79) | 251,710,934 → 250,662,310, then 247,123,864 → 246,304,376 on their respective controls. |
| Borrow embedded Vesta SRS | [r76–r78](round-index.md#r78) | The first two implementations regressed; the outlined r78 form retained 250,662,310 → 247,123,864. |
| Proof-required wrap SRS prefix and exact IPA reservation | [r82/r83](round-index.md#r82) | 232,979,444 → 231,063,964 → 229,783,781. Logical domain size and logical scalar/point lengths remain intact. |
| Canonical accumulator / IPA coefficient buffers | [r85](round-index.md#r85), [r87](round-index.md#r87) | 229,783,781 → 227,621,594; 224,885,894 → 223,445,596. Preserve canonicality and fallback for other fields/batch shapes. |
| Fixed Vesta positional tables and coupled access | [r89/r90](round-index.md#r89) | 223,445,596 → 198,484,015 → 186,205,116. Includes build-time table derivation and selected carry geometry. |
| Borrow positional scalars; pack Pallas halves | [r91/r92](round-index.md#r91) | 186,205,116 → 182,338,026 → 179,452,923. These storage/layout choices compose with the fixed-table paths. |
| Fixed Pallas positional table | [r93](round-index.md#r93) | 179,452,923 → 159,244,851. Table identity and geometry remain bound to the actual embedded SRS. |
| Borrow IPA point prefix | [r95](round-index.md#r95) | 159,244,851 → 157,442,577. Segmented callback borrows the prefix while h, tail and scalar order remain complete. |
| Borrow embedded Pallas SRS | [r102](round-index.md#r102) | 153,727,101 → 151,858,777. Preserve Vec-compatible serialization, owned deserialization and copy-on-write mutation. |

Figures are historical 577-byte receipt-ABI measurements with distinct controls;
there is no new per-feature ablation for the current boolean guest.

## Implementation and correctness

- [IPA](../../vendor/proof-systems/poly-commitment/src/ipa.rs),
  [canonical coefficients](../../vendor/proof-systems/poly-commitment/src/canonical_ipa.rs),
  [generator storage](../../vendor/proof-systems/poly-commitment/src/generators.rs), and
  [commitment helpers](../../vendor/proof-systems/poly-commitment/src/commitment.rs).
- Root [serialization](../../crates/pickles-verifier/src/serialize.rs),
  [canonical coefficients](../../crates/pickles-verifier/src/canonical_coefficients.rs),
  [fixed Vesta tables](../../crates/pickles-verifier/src/fixed_srs.rs),
  [fixed Pallas tables](../../crates/pickles-verifier/src/fixed_pallas.rs), and
  [guest setup builder](../../crates/o1-verifier/build.rs).
- Tests: `srs_storage`, `ipa_borrowed_prefix`, `fixed_pallas_prefix`,
  `canonical_ipa`, `b_poly`, `positional_recoding` and the verifier's embedded-setup tests.

## Corrections and upgrade boundary

[r52](round-index.md#r52) is a required correctness correction, not an optional
regression to optimize away. A short prefix must never satisfy a full-domain
request. The r83 reproducer triggers the expected buffer-growth assertion before
the final MSM; this is a performance-contract failure, not proof rejection.
Ordinary verification materializes ordinary points; only trusted program
code selects the segmented opening path.

On proof-systems 0.7, caches follow upstream's Arc-backed host / Rc-backed no-std
ownership. Prefix seeding goes through `set_lagrange_basis_prefix`; no caller
accesses private cache internals. Rebase the borrowed storage and callbacks onto
new APIs while retaining old wire encoding and ownership/lifetime guarantees.
[r115–r117](round-index.md#r115) scalar-buffer alternatives remain rejected.

## Retained controls and commit ownership

| Setting | Current value |
| --- | --- |
| `MIZU_APPEND_B_POLY` | `1` |
| `MIZU_BORROW_PALLAS_SRS` | `1` |
| `MIZU_BORROW_VESTA_SRS` | `1` |
| `MIZU_BORROW_POSITIONAL_SCALARS` | `1` |
| `MIZU_FIXED_PALLAS_SRS` | `1` |
| `MIZU_FIXED_VESTA_COUPLED` | `1` |
| `MIZU_FIXED_VESTA_SRS` | `1` |
| `MIZU_CANONICAL_ACCUMULATOR_COEFFICIENTS` | `1` |
| `MIZU_IPA_BORROW_SRS_PREFIX` | `1` |
| `MIZU_IPA_CANONICAL_PREFIX` | `1` |
| `MIZU_IPA_DIRECT_COEFFICIENTS` | `1` |
| `MIZU_IPA_EXACT_CAPACITY` | `1` |
| `MIZU_WRAP_SRS_PREFIX` | `1` |

These controls can compose or select fallback paths. Their grouping assigns a
primary documentation area, not independent additive performance credit. Exact
source references are in [the file appendix](../file-changes.md).

Pinned topic commits (in their respective repositories):

- proof-systems `af2c000a7` — Add canonical IPA and reusable SRS representations.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.

## OpenVM-inspired follow-up

The opt-in [combined-accumulator experiment](openvm-experiments.md) reuses the authenticated fixed Vesta tables across a transcript-weighted multi-proof equation. The eight-proof CPU workload measured 825,188,525 → 342,539,805 cycles. Inputs repeat one mainnet proof; native checks also cover three distinct same-key producer proofs. This is not a fresh SP1 proof benchmark.
