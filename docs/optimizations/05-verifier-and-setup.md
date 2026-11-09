# Verifier setup, transcripts and commitment assembly

Status: retained reductions in repeated setup and commitment construction.
Precomputed values are accepted only through the trusted embedded build path;
runtime keys keep ordinary recomputation and validation.

## Work records

| Work | Historical rounds | Recorded outcome |
| --- | --- | --- |
| Avoid unused permutation-cell map | [r42/r43](round-index.md#r43) | 377,710,388 → 314,345,145. `shifts_only` computes the same coset shifts without building seven domain-sized vectors that this verifier never reads. |
| Reuse matching cached linearization | [r44](round-index.md#r44) | 314,345,145 → 307,478,839. Reuse only for matching feature configuration; other configurations rebuild. |
| Public-input / combined commitments use one optimized Pasta MSM | [r55](round-index.md#r55) | 278,339,786 → 273,543,189. Removes remaining half-MSM split and routes through the verifier hook. |
| Fuse ft commitment construction | [r56](round-index.md#r56) | 273,543,189 → 270,998,816. Fold chunk weights and t commitment into one equivalent MSM; no materialized intermediate f. |
| Build-derived authenticated VK digest | [r50](round-index.md#r50) corrected framing; [r63](round-index.md#r63) retained implementation | 266,961,479 → 264,752,510. Moving trusted fixed work out of the guest is distinct from caching a value computed once at runtime. |
| Checked host verifier-blob cache | [r67](round-index.md#r67) | Build-time improvement; zero guest-cycle saving claimed. Fingerprint executable, VK and both SRS inputs; reject corrupt/incomplete cache entries. |
| Build-derived setup/alpha registries | [r80/r81](round-index.md#r81) | 246,304,376 → 232,979,444. Precompute immutable linearization/registries with field/version/configuration checks. |

Figures above use the historical receipt ABI and each round's own control.
Setup changes can alter serialization, allocation and teardown costs; a large
cycle reduction is not automatically a reduction in field arithmetic.

## Implementation and correctness

- Root [verifier types](../../crates/pickles-verifier/src/types.rs),
  [deferred checks](../../crates/pickles-verifier/src/deferred.rs),
  [setup format](../../crates/pickles-verifier/src/setup_data.rs),
  [serialization](../../crates/pickles-verifier/src/serialize.rs) and
  [verification dispatch](../../crates/pickles-verifier/src/lib.rs).
- Kimchi [verifier](../../vendor/proof-systems/kimchi/src/verifier.rs),
  [alpha registry](../../vendor/proof-systems/kimchi/src/alphas.rs), and
  [permutation shifts](../../vendor/proof-systems/kimchi/src/circuits/polynomials/permutation.rs).
- Guest [builder](../../crates/o1-verifier/build.rs) and
  [blob cache](../../crates/o1-verifier/blob_cache.rs).
- Tests: setup serialization/configuration rejection and embedded digest
  corruption cases in the verifier, `verifier_blob_cache`, and both real-guest
  corruption controls.

## Corrections and upgrade boundary

Do not revive r50's runtime digest-cache claim: hashing already occurred once;
r63's fixed embedded digest is the retained mechanism. r49's Lagrange-cache
contract was fixed in r52, recorded under [IPA/SRS](03-ipa-and-srs.md).
Upstream 0.7 now owns `oracles_from_digest`; retain its implementation rather
than carrying a duplicate local oracle protocol. Rebuild trusted blobs after
API/setup changes; never authorize precomputed digests, callbacks or setup from
untrusted proof input.

## Retained controls and commit ownership

| Setting | Current value |
| --- | --- |
| `ZEKO_PRECOMPUTED_SETUP` | `1` |
| `ZEKO_PRECOMPUTED_VK_DIGEST` | `1` |

These controls can compose or select fallback paths. Their grouping assigns a
primary documentation area, not independent additive performance credit. Exact
source references are in [the file appendix](../file-changes.md).

Pinned topic commits (in their respective repositories):

- proof-systems `e7f63a64d` — Optimize verifier challenges, expressions and borrowed commitments.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.

## OpenVM-inspired follow-up

The [OpenVM-inspired experiments](openvm-experiments.md) add optional borrowed/fused deferred CIP schedules, complete embedded batch verification, and a separate statement/VK digest output feature. Normal single-proof input/output and retained defaults are unchanged. These Pickles-specific changes stay in the root topic commits.
