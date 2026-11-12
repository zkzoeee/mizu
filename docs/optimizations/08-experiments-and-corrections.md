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
- The 2026-11-11 gate record said sshd blocked a slim pod image and that only a baked
  image or the untested entrypoint could unblock it. Both halves were wrong about the
  remedy: Runpod documents a container start command that installs `openssh-server` at
  boot, `create-pod` takes `entrypoint`/`cmd`, and `startSsh: true` supplies the keys.
  A slim image needs no registry. The image sizes were then read from the registry
  rather than assumed: `nvidia/cuda:12.8.1-base-ubuntu24.04` is 101 MB compressed and
  carries `cuda-cudart-12-8`, against the PyTorch image's 10,564 MB. Measured the
  same day in [mizu-slim-pod-image](../../benchmarks/gpu-slim-base-image.json): ready
  22 s after create against ~112 s, nothing unresolved in `ldd`, and the same accepted
  proof. Still open: that 2 s pull was a warm machine cache.
- Neither catalog read is authoritative, and they contradict each other. Within minutes
  on 2026-11-11 the fleet-wide `get-capacity` graded the A5000 `Low @ $0.27` on 13.2
  while a create in EU-SE-1 was refused and `get-gpu-type` reported `NONE` with no data
  centres. The fleet-wide grade is not data-centre scoped; the per-GPU field spans
  Community. A network volume pins the pod to one data centre, so stock elsewhere buys
  nothing. Use the probe to decide whether to keep waiting, and the create to decide
  anything else. Sharper instance the same evening: `get-gpu-type` graded the A6000
  `LOW` with `dataCenters: [{EU-SE-1, LOW}]` -- scoped to the data centre, so not
  excusable as fleet-wide staleness -- and five creates were refused in the next two
  minutes, one of them carrying no CUDA constraint at all. Pushed further the same
  evening, the request was stripped one axis at a time -- no volume, no data-centre
  filter, no CUDA constraint, Community instead of Secure, and finally a bare pod with
  none of them -- and all eleven creates were refused while both reads still said
  `LOW`. So an availability grade can be unconditionally false, not merely stale or
  mis-scoped. A grade is not a promise on any card class; budget for the create to
  fail, and never wait on a catalog field. The converse closed it on 2026-11-12: the
  A40's `LOW` in EU-SE-1 -- the same grade that had refused creates the day before --
  took the very first create, after forty-nine refusals across eighteen hours. So the
  grade is advisory in BOTH directions. It neither promises capacity nor denies it, and
  a `LOW` that refused an hour ago is still worth one attempt.
- **The A40 pool is not uniform: six hosts agree to 3.4% and a seventh was 11.8%
  faster, so cross-pod timings are still not comparable -- for a different reason than
  I first wrote.** The first version of this bullet had n=2 and read the gap as
  symmetric host noise, "at least 12.5% variance on any A40 timing". Tabulating all ten
  recorded measurements of the retained arm at `PASTA_R5_INFLIGHT=8` refutes that
  shape. Seven A40 runs across five cards: six of them, including two repeat pairs on
  one card, fall in 107.74-111.36 s, a 3.4% band; the seventh (`7d6ed7f9`, 2026-11-12)
  proved in 96.32 s, 11.8% below that cluster's median and 10.6% below even its fastest
  member. The work was bit-identical throughout -- same `elf_sha256`, same
  `verification_key_sha256`, 142,638,298 cycles, 82 shards, same inflight depth and
  worker count. So a repeated measurement on one card is good to ~3%, and the risk is
  not noise but drawing the wrong card: the outlier reported 50,899,386,368 bytes of
  total VRAM against the cluster's 47,699,722,240, and the record holds only those two
  values. Against the nominal 48 GiB the outlier is 1.2% short and the cluster 7.5%
  short, while all three A5000s report an identical 25,285,623,808 bytes, 1.9% short of
  their nominal 24 GiB -- so the extra ~6.3% on the cluster is the size of the ECC
  reservation. **Hypothesis, unmeasured**: the fast host
  had ECC off, which would also explain the bandwidth. `nvidia-smi
  --query-gpu=ecc.mode.current` on the next pod decides it. The operative rule is
  unchanged -- every per-arm comparison in these records is measured on one pod in one
  session, and an arm-versus-arm claim built from two pods must be rejected -- but the
  number attached to it was wrong, and so was its character.
- **The A5000's speed advantage does stand; I retired it on n=1 and had to restore
  it.** On 2026-11-12 I concluded from the single fast A40 run that the A5000's
  128.43 s "sits inside the A40 range" and that only its price advantage survived. The
  full set says otherwise: three A5000 runs on three cards give 104.09/106.88/108.23 s,
  median 106.88 s, which is 2.1% faster than the A40 cluster's 109.15 s median and
  faster than every A40 except the ECC outlier. At $0.27/hr against $0.59 that is
  $0.0080 of prove-loop cost against $0.0179, 55% cheaper per proof. What is genuinely
  dead is the stronger original claim that it beat *every* A40 run. The lesson is the
  same in both directions: a conclusion drawn against one new measurement is worth
  exactly one measurement.
- **Measured pod cost, from the billing API rather than from arithmetic.** The eleven
  rentals of the campaign billed $1.6196 in all (GPU $1.5893, disk $0.0302) against a
  $2.00 cap. The five cache-warm single-proof rentals cost $0.0225 to $0.0759 end to
  end; the cheapest complete verified proof was 2.3 cents, on an A5000 under the slim
  base image. Per-pod figures confirm the earlier pod-life arithmetic to the cent
  ($0.0452 and $0.0530 for the two end-to-end timed runs). The proof is 37-50% of a cache-warm
  rental; the largest single variable is Runpod's create-to-ready
  handover, 18 s on one run and 67 s on another. Pods terminate out of the API, so
  `get-pod` 404s afterwards and `list-pod-billing` with `bucketSize: hour` is the only
  retrospective source -- attribution back to a run is by clock and by rate arithmetic.
- **An exact-string arch comparison is the wrong predicate, and both scripts said so
  in opposite directions.** The cache probe compared `cuda_archs` as a string while
  `push-and-launch.sh` defaulted it to `86` and the EU-SE-1 cache was built fat for
  `86,89,120`, so a warm cache read as a miss, the launcher chose the build path, and
  the build died immediately on a slim image with no `nvcc` -- 20 s of pod time for
  nothing. `pod-prove.sh`'s own header had documented the trap ("the key is exact, so
  an auto-detected 86 misses an 86,89,120 cache") and warned about it at runtime; the
  launcher's default contradicted it. Knowing a trap and encoding it in a default are
  different things. Fixed on 2026-11-12: `archs_cover()` asks whether the stored build
  carries every architecture the card needs, in that direction only (a binary built for
  `86` does not run on an `89` card), the stored key stays an exact hash because it is
  an identity and not a compatibility test, and the launcher reads `compute_cap` from
  the pod instead of assuming. Eleven cases checked, including every must-miss
  direction and a moved source fingerprint; a slim pod still misses the full scope on
  `nvcc` and hits the runtime scope, which is the intended trade.
- "Retry the create, never re-read availability" was half a rule. It is right for a
  `LOW` pool, and useless against an empty one: the A5000 refused fourteen creates on
  2026-11-11 while its per-GPU read said `LOW`, because that figure spans Community
  too, and `get-capacity` with explicit `cudaVersions` reported `Out` on 12.8, 13.0
  and 13.2 in Secure EU-SE-1. Retry a few times, then probe.
- `python3-minimal` has no `json` module. The prove path installed it from the day it
  was written and had never met an image without python3, so the gap was invisible
  until a 101 MB image met it: the cache probe raised `ModuleNotFoundError`, the
  launcher read it as a cache miss, and the nvcc guard refused to build on a pod with
  no compiler. A dependency that is only exercised on one image is not tested.
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
- "The executor's ~155 s is the irreducible floor of a proof" was wrong, and
  stated in this session before it was measured. About 72 s of it was the
  producer blocked on a two-deep record channel. Depth 8 removes 26 s of wall;
  see [mizu-record-channel-depth](06-sp1-runtime-and-proving.md#mizu-record-channel-depth-the-record-channel-was-the-bottleneck-not-either-side).
- "The GPU is idle most of the run, so a faster card buys nothing" holds at
  channel depth 2 and is weaker at depth 8: device busy time is unchanged at
  68-69 s but is now 51% of the wall rather than 42%, and the card's idle
  fraction falls from 57% to 49%. The claim must be quoted with the depth.
- Porting wazabi r125's `PASTA_DEPS_BATCH` was listed as the best next lever
  and turned out to be **already implemented**: the fork's pipelined producer
  generates dependencies concurrently across a chunk's records, one thread per
  record, and emits in execution order. Measured locally on 2026-11-11, the
  phase scales **1.82x on 8 threads** with `global_dependencies_opt` on (43%
  serial by Amdahl) and **1.23x** with it off (78% serial). The far bigger
  lever is that setting itself: dependencies are 225.0 s at one thread with it
  off against 50.6 s with it on, and it is already true in both manifest arms.
  Evidence: [producer-dependency-scaling.json](../../benchmarks/producer-dependency-scaling.json).
- The GPU prove-loop pipelining (`PASTA_PROVE_INFLIGHT`, r120's ranked idea 3)
  was written and defaulted to 2 before any GPU had run it. The first real GPU
  proof from this repository (2026-11-10) found that two shards in flight panic
  in `sp1-gpu-logup-gkr`'s `generate_first_layer`; the default is now 1, which
  is serial, and the idea is open rather than retained. Source presence of the
  pipeline does not imply selection.
- "Retention saves about 12% of proving wall clock" is r120's figure on r120's
  host. The same comparison on an A40 pod with a Xeon Gold 6342 gives −24.4%
  from the same −44.6% cut in shard count, so the saving converts to wall at a
  rate the host sets. Quote either figure with its host; see
  [mizu-gpu-core-proof-a40](06-sp1-runtime-and-proving.md#mizu-gpu-core-proof-a40-the-first-real-gpu-proof-from-this-repository).
- A `--features cuda` path that has never been compiled on Linux is a draft,
  not code. That build found three defects in sequence in this repository, each
  the same kind wazabi's r123 found in its own tree: a resolver-2 feature gap
  that starved a build script, a missing `type Gpu` whose doc comment had been
  ported without its declarations, and `SP1CoreOpts` moved before its own
  guards read it. None of the three can fire on macOS.
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
