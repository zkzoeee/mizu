# Field arithmetic and representations

Status: retained, with dispatch restricted to the supported Pasta fields and
configurations. These changes span the algebra fork, reusable curve code in
proof-systems and Pickles-specific preparation in the root verifier.

## Work records

| Work | Historical evidence | Retained form / decision |
| --- | --- | --- |
| Canonical Vesta/Pallas coordinates and zero filtering | [r1–r2](round-index.md#r1): canonical/filter combination 5,066,609,487 → 3,997,137,768 cycles; later combined r2 winner 2,895,176,178 | Keep long MSM computations in canonical residues; do not apply canonical limbs as if they were Montgomery values. |
| Direct modular multiplication and checked inverse hints | [r3](round-index.md#r3): direct + hints 2,895,176,178 → 2,380,124,270; hints alone only ~0.07% | Use existing constrained UInt256 arithmetic; check hint canonicality and the inverse equation. Hints are not trusted results. |
| Remaining Montgomery multiplication/squaring | [r4](round-index.md#r4): direct Montgomery variant 2,381,000,358 → 2,258,258,011; combined r4 winner 1,974,602,537 | Exact modulus guards and prepared operands in Arkworks; retain the squaring predicate correction. |
| Redundant conversion trimming | [r22](round-index.md#r22): 545,144,012 → 536,516,286 | Skip the unused bigint buffer on eligible GLV paths. Earlier r4 trimming results do not predict the later backend's gain. |
| Fused Vesta / Pallas base conversion | [r65](round-index.md#r65): 264,752,510 → 258,722,659; [r86](round-index.md#r86): 227,621,594 → 224,885,894 | Feed final coordinate/GLV storage directly, avoiding intermediate representation buffers. |
| Serial zkVM schedules | Current algebra topic stack | MSM, batch inversion and polynomial evaluation avoid thread-dependent schedules in the guest; this is not a separately timed historical round. |

All figures above are historical 577-byte receipt-ABI measurements. Each row's
control is specific to that round; do not add the reductions or project them
onto the current boolean guest.

## Implementation and correctness

- [Montgomery backend](../../vendor/algebra/ff/src/fields/models/fp/montgomery_backend.rs),
  [batch inversion](../../vendor/algebra/ff/src/fields/mod.rs), and
  [polynomial evaluation](../../vendor/algebra/poly/src/polynomial/univariate/dense.rs).
- [Canonical Pasta words](../../vendor/proof-systems/curves/src/pasta/canonical_words.rs),
  [canonical Pallas](../../vendor/proof-systems/curves/src/pasta/canonical_pallas.rs),
  [checked hints](../../vendor/proof-systems/curves/src/pasta/precompile_spikes.rs), and
  [root canonical MSM preparation](../../crates/pickles-verifier/src/canonical_msm.rs).
- Tests: `mont_direct_spike`, `precompile_spikes`, `pallas_field_spikes`,
  `msm_spikes` and the real fixture/negative guest executions.
- `canonical_words_fuzz` (added 2026-11-10) is the seeded differential for the
  canonical residues against arkworks, over boundary classes and random
  operands. On the host their products are arkworks, so what it covers is the
  logic around them: the conditional subtraction in `add_assign`, the `pow7`
  schedules, and the wide accumulation, `2^256 mod p` folding and up-to-three
  reductions of the three-term inner products. See
  [measurement and tooling](07-measurement-and-tooling.md#mizu-fuzz-expansion-five-more-fuzz-surfaces-and-the-argument-channel).

Arkworks' inverse backend itself still delegates to its configured inverse;
checked guest inverse hints and host witness inversion are different mechanisms.
The host witness work is recorded under [SP1](06-sp1-runtime-and-proving.md).

## Alternatives and upgrade boundary

[r15](round-index.md#r15) found `mina-curves/sp1` +4,289 cycles on its control;
that Cargo feature remains off in the retained core build. Exact Comba high
products ([r24](round-index.md#r24)) regressed by 58,982,244 cycles. Do not revive
those alternatives just because multiplication or allocation counts look lower.
When rebasing, review field representation, exact modulus selection, syscall ABI,
Montgomery constants and differential arithmetic tests together. Upstream 0.7
already supplies the field hooks; our fork adds the remaining optimizations.

## Retained controls and commit ownership

| Setting | Current value |
| --- | --- |
| `ZEKO_CANONICAL_MSM` | `1` |
| `ZEKO_CANONICAL_PALLAS` | `1` |
| `ZEKO_DIRECT_MULMOD` | `1` |
| `ZEKO_HINTED_INVERSE` | `1` |
| `ZEKO_MONT_DIRECT_MULMOD` | `1` |
| `ZEKO_TRIM_CONVERSIONS` | `1` |
| `ZEKO_FUSE_PALLAS_BASE_CONVERSION` | `1` |
| `ZEKO_FUSE_VESTA_BASE_CONVERSION` | `1` |

These controls can compose or select fallback paths. Their grouping assigns a
primary documentation area, not independent additive performance credit. Exact
source references are in [the file appendix](../file-changes.md).

Pinned topic commits (in their respective repositories):

- algebra `7b99b0e0f` — Add SP1 field arithmetic and inversion fast paths.
- algebra `8bcb64d52` — Optimize polynomial evaluation for the verifier.
- proof-systems `9f37ee019` — Add canonical Pasta arithmetic, GLV and SP1 point operations.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.

## OpenVM-inspired follow-up

The opt-in [OpenVM-inspired experiments](openvm-experiments.md) evaluate stack cross-field encoding and fused CIP products using the existing algebra primitive. No algebra fork change was made; the simpler allocation variant beat fused CIP on cycles and the SP1 gas estimate.
