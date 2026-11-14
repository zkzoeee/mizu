# Pasta MSM, GLV and memory scheduling

Status: retained shared Pallas/Vesta MSM backend. Current retained windows are
Pallas 13 and Vesta 16, with adaptive behavior for small MSMs. Scalar bounds,
signed-digit carries, exceptional points and fixed-table geometry are part of
correctness, not optional tuning assumptions.

## Work records

| Work | Historical rounds | Recorded outcome |
| --- | --- | --- |
| Canonical coordinates, affine reduction, signed buckets and GLV | [r1–r5](round-index.md#r2) | Early arithmetic baseline; custom Pasta operations then move the r4 combined 1,974,602,537-cycle guest to 621,941,587 with the 16/13 pair. |
| Window-major digits | [r18/r20](round-index.md#r20) | One change: 621,941,587 → 594,350,168. r18's findings and r20's ledger describe the same improvement. |
| Avoid positive-digit point copies | [r21](round-index.md#r21) | 594,350,168 → 545,144,012. Borrow a positive base; materialize negation only where required. |
| Flag-only bucket reset and cached negatives | [r25](round-index.md#r25), [r26](round-index.md#r26) | 536,083,919 → 517,077,466 → 478,237,799. Identity coordinates remain unread until initialized. |
| Hoisted limbs/stride/modulus, indexed comparisons, straddle guard and const recoding | [r27–r31](round-index.md#r27) | 478,237,799 → 427,791,614 across the sequence. The final kernel retains the mechanisms, not every intermediate layout. |
| Bound-derived digit slots, two-limb Vesta GLV, prepared builder, i16 digits and fixed bucket view | [r35–r41](round-index.md#r35) | The A/B/C/D series reduced the fixed 427,791,614 control; r41 corrects carry/bound claims. Earlier Pallas r40 shortening was superseded by r58. |
| Centered Pallas two-limb decomposition | [r58](round-index.md#r58), corrected inventory [r60](round-index.md#r60) | 268,092,468 → 266,961,479. Preserve the mathematical bound; r60 retracts the inferred width-8 cost explanation. |
| Packed Pallas GLV halves | [r92](round-index.md#r92) | 182,338,026 → 179,452,923; related fixed-table/scalar storage work also appears in the IPA/SRS record. |
| Remove digit zero-fill | [r98](round-index.md#r98) | 157,442,577 → 157,095,318, with every consumed slot written before use. |
| Whole-MSM boundaries | [r101](round-index.md#r101) | 157,095,318 → 153,727,101. Keep the combined outlined configuration; separate outline gains are not additive. |
| Copy only known-finite coordinate words | [r111](round-index.md#r111) | 151,858,777 → 142,692,781. Copy eight words and set the finite flag; this is distinct from r97's rejected identity-copy hypothesis. |

These are historical SP1 6.1 receipt-ABI results. Current retained arithmetic
call counts are Pallas add/double 697,988 / 369, Vesta add/double 1,310,683 / 112;
they remain identical after the proof-systems 0.7 migration.

## Implementation and correctness

- [Shared point/MSM kernel](../../vendor/proof-systems/curves/src/pasta/point_spikes.rs).
- [Pallas GLV](../../vendor/proof-systems/curves/src/pasta/pallas_glv.rs) and
  [canonical Pallas adapter](../../vendor/proof-systems/curves/src/pasta/canonical_pallas.rs).
- [Vesta GLV](../../crates/pickles-verifier/src/vesta_glv.rs),
  [Vesta adapter](../../crates/pickles-verifier/src/vesta_spikes.rs), and
  [canonical MSM](../../crates/pickles-verifier/src/canonical_msm.rs).
- [Arkworks MSM](../../vendor/algebra/ec/src/scalar_mul/variable_base/mod.rs) and
  [GLV support](../../vendor/algebra/ec/src/scalar_mul/glv.rs).
- Tests: `point_spikes`, `pallas_spikes`, `pallas_msm_spikes`, `msm_spikes`,
  `positional_recoding`, `fixed_pallas_prefix` and `pasta_input_contract`.

Keep zero/identity/equal/opposite points, signed carry boundaries, shorter-slice
semantics and differential scalar reconstruction covered. For uninitialized
storage, every read must remain dominated by the corresponding write.

## Alternatives and upgrade boundary

Window retuning in r17/r50/r51/r59/r60/r73/r112 established configuration-specific
tradeoffs. Lower stored cells at a smaller window are not evidence of faster
proofs. Epoch buckets (r99/r99b), four-lane Vesta (r100) and sorted chain grouping
(r108) regressed. Their records remain in the [experiment register](08-experiments-and-corrections.md).
Re-measure compiler/layout changes on the combined selected backend, and keep
recoder geometry equivalent before attributing a window comparison.

## Retained controls and commit ownership

| Setting | Current value |
| --- | --- |
| `MIZU_PALLAS_AFFINE` | `1` |
| `MIZU_PALLAS_GLV` | `1` |
| `MIZU_PALLAS_POINT_PRECOMPILE` | `1` |
| `MIZU_PALLAS_WINDOW` | `13` |
| `MIZU_VESTA_AFFINE` | `1` |
| `MIZU_VESTA_GLV` | `1` |
| `MIZU_VESTA_POINT_PRECOMPILE` | `1` |
| `MIZU_VESTA_WINDOW` | `16` |
| `MIZU_MSM_FILTER_ZERO` | `1` |
| `MIZU_INLINE_FINITE_COPY` | `words` |
| `MIZU_UNINIT_MSM_DIGITS` | `1` |
| `MIZU_OUTLINE_PALLAS_BUCKETS` | `1` |
| `MIZU_OUTLINE_VESTA_BUCKETS` | `1` |
| `MIZU_PACKED_PALLAS_HALVES` | `1` |

These controls can compose or select fallback paths. Their grouping assigns a
primary documentation area, not independent additive performance credit. Exact
source references are in [the file appendix](../file-changes.md).

Pinned topic commits (in their respective repositories):

- algebra `712a6ae79` — Add GLV and fixed/variable-base MSM optimizations.
- proof-systems `9f37ee019` — Add canonical Pasta arithmetic, GLV and SP1 point operations.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.
