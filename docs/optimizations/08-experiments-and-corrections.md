# Rejected experiments, prototypes and corrections

These records prevent old hypotheses and partial implementations from becoming
implicit defaults during a rebase. Historical measurements stay useful even when
the code was reverted. The current retained recipe is authoritative for enabled
compile-time settings; source presence alone does not establish selection.

## Decision register

| Candidate | Historical evidence | Decision at this audit |
| --- | --- | --- |
| Early prepared Vesta lanes and global window forcing | [r1/r2](round-index.md#r2) | Initial 2/4-lane tables cost extra cycles/image bytes. Later fixed positional tables (r89+) are a different, measured retained design. |
| `mina-curves/sp1` feature | [r15](round-index.md#r15) | +4,289 cycles; off in current retained core. Field hooks remain available upstream. |
| Exact Comba high product | [r24](round-index.md#r24) | +58,982,244 cycles; rejected. Arithmetic work outweighed removed memory operations. |
| Fused early preparation / early zero filter | [r32](round-index.md#r32), [r47](round-index.md#r47) | Rejected on their controls; later builders/prefix elimination are separate mechanisms. |
| Early Pallas shortening and inferred width costs | [r40/r58/r60](round-index.md#r60) | Final centered decomposition retained; earlier variant/explanations superseded. |
| Runtime VK digest caching | [r50](round-index.md#r50) | No retained win: digest was already computed once. Trusted build-derived digest r63 is separate. |
| Initial borrowed Vesta storage | [r76/r77/r78](round-index.md#r78) | Reject first/always-inline forms; retain outlined form. |
| Lazy identity-copy hypothesis | [r97](round-index.md#r97) | Candidate 157,156,863 looked lower than 157,442,577, but proposed hot identity predecessor was not executed. Not retained; r111 concerns finite points. |
| Epoch bucket tags/buffers | [r99/r99b](round-index.md#r99) | 176,742,210 / 170,484,757 vs 157,095,318; rejected. |
| Four-lane Vesta | [r100](round-index.md#r100) | 174,597,355 vs 157,095,318; cycle-rejected. |
| Sorted chained-add grouping | [r108](round-index.md#r108) | Best measured chain8 226,588,102 vs 151,858,777; cells also increased. No integrated chain opcode/AIR. |
| Compressed SRS | [r109](round-index.md#r109) | Standalone encoding/AIR prototype in tools; hypothetical blob saving is not a production guest or proof gain. |
| Poseidon round accelerator | [r110](round-index.md#r110) | Standalone AIR/model in tools; missing complete state/memory/syscall integration. No current guest-cycle claim. |
| Later specialized window alternatives | [r73/r112](round-index.md#r112) | Keep V16/P13 cycle winner. Smaller windows can reduce record cells while increasing cycles; no proof-time winner inferred. |
| IPA scalar ownership/compaction | [r115](round-index.md#r115) | 143,771,553 vs 142,692,781 (+1,078,772); rejected. |
| IPA original scalar slots | [r116](round-index.md#r116) | 145,344,914 (+2,652,133); rejected. |
| Borrowed IPA scalar halves | [r117/r118](round-index.md#r117) | 143,803,681 (+1,110,900), cells also worse; rejected/restored. |
| Dependency batch width 8 | [r125/r126](round-index.md#r125) | External, not imported. GPU pipeline wall 145.70 → 145.00 s, RSS 8.06 → 16.37 GB; upstream sweep changed Rayon width with batch size. r125 says do not ship. |
| Consumer split | [r127](round-index.md#r127) | Manifest/scripts only at capture; no completed evidence or import. |

All candidate cycle figures above are historical receipt-ABI measurements.

## Corrections that must travel with the result

- r18 and r20 describe one window-major improvement; do not count twice.
- r8/r9 qualify early r6/r7 proof claims with chip inventory, complete execution
  verification and independently derived keys; per-shard checks were insufficient.
- r35/r41 qualify decomposition timings, carry bounds and earlier claims.
- r49's short-prefix/full-domain cache contract was corrected by r52; the current
  implementation retains distinct caches.
- r50/r51/r59/r60 qualify window sweeps and code-generation effects.
- r53's accounting was corrected by r54, then extended with the matched v3
  producer in r71/r72. Old inflated totals are not authoritative.
- r83's capacity reproducer intentionally fails the no-growth assertion before
  the final MSM; this is distinct from a cryptographic verifier rejection.
- r97's hypothesis was not established by its smaller cycle number.
- r124 replaces cross-host SP1 timing attribution with a matched-host comparison.
- r125 corrects r126 despite the numerical round order. Round numbers alone are
  not a chronology or a dependency graph.
- The first `pasta-fuzz` run (2026-11-10) reported three seeds but ran one: the
  seed parser read decimal only and hex seeds fell back to the default. The
  parser now accepts `0x` values and fails on bad input; the second run in the
  [verification plan](../pasta-air-verification-plan.md) is the first with
  distinct seeds.
- Two limits stated with those runs were superseded the same day by the
  argument channel in
  [mizu-fuzz-expansion](07-measurement-and-tooling.md#mizu-fuzz-expansion-five-more-fuzz-surfaces-and-the-argument-channel):
  a column surviving the sweep is not thereby unconstrained, and Poseidon P4's
  memory-only class is now checked rather than deferred. The earlier survivor
  sets stand as measured; what changed is what they mean.
- The first version of the executor differential compared the minimal
  executor's result words against an empty list for the field operations,
  because their events are filed under the add syscall code, and so passed
  without comparing anything. Its baseline case now asserts that the
  well-formed call produced one event and the same words on both sides.

The [catalog](../../benchmarks/optimization-history.json) preserves these
relationships and hashes of the original evidence. A future attempt should cite
its predecessor, explain the changed premise, use a fresh matched control and
update this record with its outcome. A prototype becomes retained only after its
missing integration and correctness gates have explicit evidence.

## Retained controls and commit ownership

No additional setting from the 40-flag retained recipe is assigned to this area.
Optional harness/routing/GMP settings do not become defaults through this record.

Root-specific code stays in the o1js-to-zkvm module series. Current pins and
ordered commits are in [the dependency lock](../../dependencies/lock.json).
Historical evidence and current implementation are connected by this record,
not by preserving the original experimental commit history.

## OpenVM-inspired follow-up

The [fused CIP prototype](openvm-experiments.md) removes 66 UInt256 calls versus the allocation candidate but adds 3,189 guest cycles and 1,005 units in the SP1 gas estimate. Keep the measured regression and raw data; do not select it from call counts alone.

## Post-retention ceilings: Poseidon permutation chip and chained Pasta add

Assessed 2026-11-10 against the retained boolean guest under the promoted
`retain-pasta-and-uint` routing (27.84B main cells, 82 records):

| Chip | Main cells | Share |
| --- | ---: | ---: |
| Global | 17,454,646,720 | 62.7% |
| VestaAddAssign | 2,096,608,800 | 7.5% |
| PallasAddAssign | 1,116,536,928 | 4.0% |
| MemoryLocal | 627,776,640 | 2.3% |
| Uint256MulMod | 198,119,936 | 0.7% |

- **Chained Pasta add (`DESIGN-batched-pasta-add.md`): not pursued.** Its
  1.5x case was the memory argument for operand words under deferred routing.
  Retention already keeps add operands in CPU shards, where repeated bucket
  accesses deduplicate; every addition still needs its own 1,599-column row.
  What a chain could still remove is part of the residual operand traffic,
  bounded by MemoryLocal's 2.3% plus the adds' share of Global, against a new
  wide AIR at the 8,192-column ceiling with intermediate exceptional cases.
  It is also not a cycle win: the guest would have to group additions by
  bucket, and r108 measured that grouping at +74.7M to +78.3M cycles with
  existing single adds. Revisit only if a measured per-chip split shows the
  adds' Global share is large.
- **Poseidon permutation chip: built later the same day** as
  [`mizu-pasta-poseidon-precompile`](04-poseidon.md#mizu-pasta-poseidon-precompile-the-kimchi-permutation-as-one-syscall)
  (−6.62% cycles, −5.22% main cells against the Pasta Fp control; 25.95M cells
  for the new chips, close to the estimate below). The assessment as written
  before it was built: r110
  measured about 11.0M guest cycles (7.7%) for the 229 permutations, with
  264,495 field multiplications. A round-per-row chip linked by interactions
  needs about 12,600 rows of roughly 2,600 columns (~33M cells), against an
  estimated 1–2B cells and 7% of cycles today. It needs a new syscall, round
  linkage through interactions (SP1 6.x has no next-row transitions), state
  memory binding and its own adversarial proof matrix. It is a separate
  multi-day project; the r110 prototype in `tools/pasta-proof` is its start.
