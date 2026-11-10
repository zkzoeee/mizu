# Optimization work records

These records group the completed optimization work by area and connect it to
its original numbered experiments. They describe implementation at `1a71eef`
(proof-systems 0.7.0 / SP1 6.10.0 / Arkworks 0.5.0). All 40 retained benchmark
settings have one primary area below; many settings affect multiple modules.

| Area | Coverage |
| --- | --- |
| [Field arithmetic and representations](01-field-arithmetic.md) | 16 related rounds; 8 primary retained settings |
| [Pasta MSM, GLV and memory scheduling](02-pasta-msm.md) | 49 related rounds; 14 primary retained settings |
| [IPA, challenge coefficients and SRS storage](03-ipa-and-srs.md) | 31 related rounds; 13 primary retained settings |
| [Poseidon, constants and scalar challenges](04-poseidon.md) | 8 related rounds; 3 primary retained settings |
| [Verifier setup, transcripts and commitment assembly](05-verifier-and-setup.md) | 15 related rounds; 2 primary retained settings |
| [SP1 execution, AIRs and host proving work](06-sp1-runtime-and-proving.md) | 33 related rounds; 0 primary retained settings |
| [Measurement, audits and development tooling](07-measurement-and-tooling.md) | 49 related rounds; 0 primary retained settings |
| [Rejected experiments, prototypes and corrections](08-experiments-and-corrections.md) | 40 related rounds; 0 primary retained settings |

## Evidence and current baseline

- [Round index](round-index.md): 125 numbered rounds, with links to detailed
  reports; r16/r19 have no located numbered record, and r127 has only a manifest
  and scripts at capture.
- [Machine-readable catalog](../../benchmarks/optimization-history.json): 246
  original source-file fingerprints, historical conclusions and selected metric
  excerpts, area membership, corrections, exact recipe and local checkpoints.
- [Current full measurement](../../benchmarks/core-proof-systems-0.7-sp1-6.10.json)
  and [upgrade validation](../proof-systems-upgrade.md): **142,650,295 cycles**,
  valid proof input, output `01`, 249 native tests and two rejected corruptions.
- [Dependency/version inventory](../dependencies-and-optimizations.md),
  [complete changed-file list](../file-changes.md), and
  [upstream sync guide](../../dependencies/README.md).

Historical full-verifier rounds mostly use a 577-byte receipt output on SP1 6.1.
Historical 6.10 ports and GPU proofs also have their own exact pins and ELF.
The consolidated core uses a one-byte boolean. Only matched controls with the
same ABI/configuration/tool identity establish an isolated cycle comparison.
The 0.4.1 → 0.7 core migration measured 142,660,047 → 142,650,295 (−9,752), with
unchanged input/output and arithmetic calls. Do not calculate an isolated gain
from the historical receipt guest to the current core.

## How to keep the records

External comparisons are kept separate from retained Mizu work. See the
[o1-openvm review](o1-openvm-review.md) for its optimization inventory, linked
dependency changes and the OpenVM/SP1 comparison.
The [Mizu experiment register](openvm-experiments.md) tracks the opt-in adaptations
and their separate validation/measurement outcomes. All 36 CPU runs passed;
combined accumulation saved 58.49% of whole-guest cycles at eight repeated
mainnet proofs. The retained configuration remains unchanged. See also the
[Miza validation review](miza-validation-review.md) for comparator, fixture and
proof-handoff lessons, and a separate pending SP1 AIR review.
The restored [cached development loop](../development-loop.md) combines targeted
native checks, independent Poseidon screening, frozen paired measurements and a
separate full CPU promotion gate. Its [validation record](../../benchmarks/dev-loop-validation.json)
distinguishes fresh executions, checked reuse and cryptographic proof gates.

For each new optimization or dependency port, update the responsible area and
add an entry following [the template](TEMPLATE.md). Include retained, rejected,
superseded, prototype, profiling-only and pending outcomes. Link the current
implementation, its topic commit and the actual evidence; keep the old result
when its conclusion is corrected. A new Mizu record uses its own stable ID;
do not consume or renumber the original workspace's r-number sequence.

A claim needs a metric and scope: guest cycles, host microseconds, record cells,
shape estimates and end-to-end proof seconds are different quantities. Never
sum variant savings measured on different controls. An enabled setting is not
proof of a separately measured saving. Builds, CPU verification, AIR row tests,
actual proof generation, from-disk verification and CUDA execution each need
their own status.

Original details remain in the sibling `wazabi/spikes` workspace. The links in
this directory point there for convenience, while the committed summaries,
selected numeric facts and hashes remain available in a standalone clone.
The consolidation recovery archive contains older source/history; newer rounds
may exist only in the original workspace. No raw experiment dumps, application
folders, credentials or pod-control scripts were imported. The historical
workspace was read only. This records a snapshot; it does not monitor r127.

The 125 records preserve selected evidence, not a full archive of every log or
variant. A SHA-256 identifies the captured working-copy file, including
uncommitted content; the original repository's HEAD alone is insufficient.

## Zakura and Ragu follow-up (2026-11-10)

Techniques adapted from Zakura Common's Pasta work, with no code or dependency
imported, are recorded in their areas: batched witness inversion, a fixed-limb
executor path and leaner field-op population in
[SP1 runtime](06-sp1-runtime-and-proving.md#mizu-zakura-host-witness-batched-inversion-fixed-limb-executor-leaner-field-ops)
(single-threaded record generation −49.3%, bit-identical witness), the opt-in
deferred MDS reduction in [Poseidon](04-poseidon.md) (+2.50% cycles, −1.20% main
cells), and a batch-transcript review in the
[experiment register](openvm-experiments.md). Ragu's verification approach is
adapted in the [Pasta AIR verification plan](../pasta-air-verification-plan.md),
whose first fuzzing layer is recorded in [measurement](07-measurement-and-tooling.md).
Evidence: [summary JSON](../../benchmarks/zakura-ragu-experiments-sp1-6.10.json).
The follow-up's Poseidon permutation chips, one syscall per kimchi permutation
with a 55-row round chain, are recorded in
[Poseidon](04-poseidon.md#mizu-pasta-poseidon-precompile-the-kimchi-permutation-as-one-syscall)
(−6.62% cycles, −5.22% main cells against the Pasta Fp precompile; opt-in).
