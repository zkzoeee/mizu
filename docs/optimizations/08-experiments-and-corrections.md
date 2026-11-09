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
