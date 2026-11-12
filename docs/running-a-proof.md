# Running a proof: machines, wall time and prices

Status: 2026-11-11. Every figure below is measured on this repository's own
pinned statement — the retained boolean guest, 142,638,298 cycles, 82 shards —
unless it says otherwise. The procedure, the traps and the driver internals are
in [the rented-GPU runbook](gpu-run-on-runpod.md); this page is the decision:
which card, how long, what it costs.

## The short answer

**Rent an RTX A5000 at $0.27/hr in EU-SE-1.** It proved the arm in **128.43 s**
— faster than the A40 at $0.59/hr — and on a warm cache a whole run is about
five minutes of pod life, so roughly **$0.022 a proof**.

```bash
scripts/gpu/make-bundle.sh /tmp/mizu-bundle.tar.gz --input /tmp/mizu-gpu-input
```

```bash
DEADLINE_MINUTES=30 RATE_PER_HOUR=0.27 CAP_USD=2.00 CUDA_ARCHS="86,89,120" ARMS="retain-pasta-and-uint" scripts/gpu/push-and-launch.sh <host> <port> /tmp/mizu-bundle.tar.gz
```

```bash
WITH_PROOFS=0 scripts/gpu/collect-results.sh <host> <port> benchmarks/gpu/runs/$(date -u +%Y%m%dT%H%MZ)
```

Create the pod in **EU-SE-1**, Secure, with the build-cache volume
`zvx1pq06vk` mounted at `/workspace`, `22/tcp` and `startSsh`, and a CUDA floor
of `12.8`. Use `nvidia/cuda:12.8.1-base-ubuntu24.04` with the start command in
[`scripts/gpu/slim-pod-start-command.sh`](../scripts/gpu/slim-pod-start-command.sh):
it was **ready 22 seconds after create**, against ~112 s on the PyTorch image,
and it proved the arm unchanged. Then terminate it: a stopped pod still bills
its disk.

## What we actually measured

Same statement, same arm (`retain-pasta-and-uint`), one proof each, all
accepted by the complete verifier and reloaded from disk:

| Card | $/hr | Wall | Executor | Prove loop | Device busy | Peak VRAM |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| **RTX A5000** | **0.27** | **128.43 s** | 120.71 s | 104.09 s | **63.0 s** | 18,893 / 24,564 |
| A40 (EU-SE-1) | 0.59 | 136.14 s | 127.01 s | 108.89 s | 68.6 s | 18,991 / 46,068 |
| A40 (EU-SE-1, cache hit) | 0.59 | 137.98 s | 129.23 s | 111.36 s | 67.6 s | 18,991 / 46,068 |
| A40 (CA-MTL-1) | 0.59 | 162.02 s¹ | 154.16 s | 137.58 s | 68.3 s | 19,256 / 46,068 |
| A40 (EU-SE-1, first run) | 0.59 | 168.63 s¹ | 159.80 s | 141.97 s | — | 18,991 / 46,068 |

¹ before `PASTA_R5_INFLIGHT=8`; the channel depth alone is −16.2%, so those two
rows are not card comparisons.

The `default` arm, for the routing comparison rather than for keeping: **223.14 s**,
148 shards, 227,918,740 proof bytes.

### Why the A5000 wins

It is the same `sm_86` architecture as the A40, so one fat binary covers both,
and it has **more memory bandwidth** (768 GB/s against 696) with fewer SMs.
Device-busy time is **63.0 s against 68.6 s, −8.2%**, and that part is cleanly
the card: both pods reported the same CPU, an Intel Xeon Gold 6342 @ 2.80 GHz.
The executor was also 5% faster, which the card cannot explain, so part of the
5.7% wall gap is host variation — there is one run per card.

What you give up is headroom: **5,671 MiB** over the peak instead of 27,000,
and no room for r128's two-stream mode, which needs 42,962 MiB and so wants a
48 GB card.

## Cards we did not run, and what the numbers imply

Prices and availability from catalogue reads on 2026-11-10/11; stock moves
within minutes, so treat availability as a snapshot and the prices as current.

| Card | VRAM | Arch | Secure | Community | Stock | Verdict for this workload |
| --- | ---: | --- | ---: | ---: | --- | --- |
| **RTX A5000** | 24 GB | sm_86 | **$0.27** | $0.16 | LOW | **the pick** |
| RTX 3090 | 24 GB | sm_86 | $0.50 | $0.22 | LOW/NONE | cheapest *gaming* card; wazabi proved Groth16 on one |
| A40 / RTX A6000 | 48 GB | sm_86 | $0.59 | $0.33–0.35 | A40 **MEDIUM** | pay this only for two streams later |
| L4 | 24 GB | sm_89 | $0.59 | $0.44 | LOW | avoid: ~300 GB/s would push device time past the executor |
| RTX 4090 | 24 GB | sm_89 | $0.89 | $0.34 | LOW, **EU-RO-1 only** | unreachable with the cache: the volume is EU-SE-1 and volumes are DC-local |
| RTX 6000 Ada | 48 GB | sm_89 | $0.99 | $0.74 | LOW | no measured advantage |
| RTX 5090 | 32 GB | sm_120 | $1.19 | $0.69 | LOW | twice the price for ≤2%: the executor is the pole |
| A100 80 GB | 80 GB | sm_80 | $1.79 | $1.19–1.39 | LOW | nothing here needs 80 GB |
| RTX PRO 6000 SE | 96 GB | sm_120 | $2.49 | $1.69 | **MEDIUM**, 11 DCs | r128 used one for its two-stream run |
| H100 / H200 / B200 | 80–180 GB | sm_90/100 | $3.99–7.99 | — | LOW | no |
| PRO 6000 MIG 24 GB | 24 GB | — | $0.69 | — | LOW | **our driver refuses MIG**: no memory or utilisation telemetry |
| V100, T4 and older | — | sm_70/75 | — | — | — | **impossible**: the fork builds 80/86/89/90 (+100/120 on CUDA ≥ 12.8) |

Three things that are settled and need not be re-argued:

- **A faster GPU buys ≤2%.** At channel depth 8 the executor (127 s) is the
  critical path and the prove loop (109 s) hides inside it. The card is idle
  about half the run.
- **More vCPU buys nothing.** 15 vCPU beat 20 in wazabi's r124, because the
  pipeline keeps only 2.5–2.6 cores busy.
- **Host clock is the biggest uncontrolled variable and you cannot order it.**
  All five of our runs drew the same Xeon Gold 6342; r120's host did identical
  work 2.3x faster than r124's. Picking a data centre does not pick a host.

## What a run costs, phase by phase

Measured on the cache-hit A40 run, which is the shape to expect:

| Phase | Seconds |
| --- | ---: |
| Create → `ssh.direct` (20 GB image pull) | ~112 |
| Create → ready on the 101 MB base image, apt and gates included | **22, measured** |
| Launcher latency, incl. 29 s upload | ~70 |
| apt + cmake + rustup (skipped on the prove path) | 22 |
| Cache check and reuse | 4 |
| Pre-flight | 1 |
| **Proof** | **139** |
| From-disk verification | 19 |
| Collect + terminate | ~60 |

So **3:03 of work inside 7:00 of billing**. A cold fat build adds **12:31**
once per source change.

| Shape | Pod life | A5000 | A40 |
| --- | ---: | ---: | ---: |
| Cold fat build + one arm | ~18 min | $0.081 | $0.18 |
| Cache hit, one arm, no download | **5.6 min, measured** | **$0.025** | $0.045 |
| Slim base image + start command | **4.67 min measured¹** | **~$0.021** | $0.046 |
| Baked image, binary inside, no volume (unmeasured) | ~3 min | ~$0.014 | ~$0.030 |

¹ measured end to end on 2026-11-11 and broken down in
[the README](../README.md#one-real-proof-on-a-rented-gpu-4-minutes-40-seconds):
18 s to ready, 139 s of proof, 20 s of verification, and 37 s of operator
latency a script would remove. The A5000 column is that duration at $0.27/hr.

**Measured on the prove-only path** (2026-11-11, A5000, cache hit): 2 min 39 s
of in-pod work, of which **7 seconds** came before proving began — cache check
4 s, reuse 2 s, pre-flight 1 s — then 131.92 s of proof and 19 s of from-disk
verification. No bundle was uploaded, nothing was installed and nothing was
compiled. Pod life 5.6 minutes, **$0.025**.

Add the volume: 10 GB `zvx1pq06vk`, billed from creation until deletion,
roughly $0.70/month at RunPod's published per-GB rate — a figure their tools do
not expose.

## The PyTorch image, and how to stop paying for it

The drivers run on `runpod/pytorch:1.0.2-cu1281-torch280-ubuntu2404` because
wazabi's r123 proved on it. Of what it provides, the prove path needs almost
none:

| Provided | Needed by | Slim alternative |
| --- | --- | --- |
| `libcudart` | the binary, always | **any CUDA image, including `base`** |
| nvcc, headers, toolkit | a **build** only | `nvidia/cuda:…-devel` |
| cmake, build-essential, git, curl | a build only, and the driver apt-installs them anyway | — |
| `python3` | the drivers' key check and JSON | `python3-minimal`, ~30 MB |
| `libgomp1`, `libstdc++6` | the binary links `-lgomp`, `-lstdc++` | one apt, a few MB |
| **torch** | **nothing, since 2026-11-11** | **`--mode cuda-probe`** |

### What the binary actually links

From `sp1-gpu-sys` 6.10's `build.rs:246-272`, which is the whole link line:
`static=sys-cuda`, then dynamically **`cudart`**, `cudadevrt` (a static device
archive), `stdc++`, `gomp`, `dl`. `CUDA::nvToolsExt` appears in that crate's
`CMakeLists.txt:150` but never on rustc's line, and CUDA 12's NVTX3 is
header-only, so it should contribute no symbol.

`libcuda.so.1` and `nvidia-smi` are **not** in any of these images: the NVIDIA
container runtime injects them from the host driver. That is why a CUDA image
with no driver in it works at all, and it is what makes the smallest tier
viable.

### The sizes, from the registry

Compressed amd64 sizes, read from Docker Hub's manifests on 2026-11-11:

| Image | Compressed | What it carries for us |
| --- | ---: | --- |
| `runpod/pytorch:1.0.2-cu1281-torch280-ubuntu2404` | **10,564 MB** | everything, including the torch we no longer use |
| `nvidia/cuda:12.8.1-devel-ubuntu24.04` | 5,162 MB | nvcc — for a **build** pod only |
| `nvidia/cuda:12.8.1-runtime-ubuntu24.04` | 2,159 MB | cudart, plus cublas/nccl/npp/cusparse we never link |
| **`nvidia/cuda:12.8.1-base-ubuntu24.04`** | **101 MB** | `cuda-cudart-12-8` and nothing else — which is all we link |

The base image's own build history confirms the one line that matters:
`apt-get install -y --no-install-recommends cuda-cudart-12-8=... cuda-compat-12-8`.
So the smallest image that can prove is **104x smaller** than the one we use.

### sshd is not a reason to build an image

This corrects what this page said earlier. Direct SSH does need `openssh-server`
*in the container*, and a stock `nvidia/cuda` image has none — but it does not
have to come from the image. RunPod's own documentation gives a container start
command for exactly this case, `create-pod` takes `entrypoint` and `cmd`, and
`startSsh: true` injects the account's registered keys as `$PUBLIC_KEY`; Secure
Cloud pods are TCP-enabled, so `22/tcp` is all that is needed in `ports`.

So sshd gets **installed at boot instead of baked in**: one apt of roughly
10-20 s in place of ~110 s of image pull, with no registry credential ever
landing on a rented host. The start command is kept in the repo as
[`scripts/gpu/slim-pod-start-command.sh`](../scripts/gpu/slim-pod-start-command.sh);
it installs `openssh-server libgomp1 libstdc++6 python3-minimal`, loads
`$PUBLIC_KEY`, runs `ssh-keygen -A` and `sshd`, then holds the container open so
`push-and-launch.sh` drives the run exactly as it does today. Nothing else in
the flow changes.

A baked image is still the fastest option — the binary is already inside it, so
there is no volume and no cache check — but it is now an *optimisation*, not a
prerequisite. `scripts/gpu/build-image.sh` builds one.

### What it did, measured

Run on 2026-11-11 on an A5000 in EU-SE-1 (pod `28694z52e90qvd`), with the
volume mounted and no image of our own:

| | |
| --- | --- |
| Image pull | **2 s** — all five layers reported `Already exists` |
| Create → ready (apt, sshd, both gates) | **22 s**, against **~112 s** to `ssh.direct` on the PyTorch image |
| `nvidia-smi -L` | answered: `GPU 0: NVIDIA RTX A5000 (UUID: GPU-0d82daa9-…)` |
| `ldd` on the cached binary | **nothing unresolved**: `libcudart.so.12` from the image, then `libstdc++`, `libgcc_s`, `libm`, `libc`, `libdl`, `libpthread`, `librt` |
| `--mode cuda-probe` | context created, 25,285,623,808 bytes of device memory |
| Proof | 135.54 s, 82 shards, 132,029,932 bytes, **accepted and reloaded** |
| Key and digest | `eed89d80…` and `4bf5122f…`, both unchanged |
| Pod life | 7.0 min, **$0.032** — of which ~2.5 min was the defect below |

`libcuda.so.1` never appears in the binary's `DT_NEEDED`: cudart dlopens the
driver, so the image needs no driver stub at all. And the proof is the same
proof — 135.54 s against the PyTorch image's 131.92 s on the same card is
+2.7%, inside the 2.6-4.2% run-to-run spread, on one run each. **The image
costs nothing in proving time; the whole saving is startup.**

A clean run of this shape is about **3.5 minutes, ~$0.016**.

### Two things this did not settle

- **The 2-second pull is a warm machine cache.** Every layer was already on
  that host. A cold pull of 101 MB is not measured — it is simply small.
- **A base image cannot build.** There is no nvcc, so the cache is still
  populated from a toolkit image; `push-and-launch.sh` now says so and exits
  before the upload instead of after it.

### The defect it found: `python3-minimal` has no `json`

The prove path had installed `python3-minimal` since it was written, and had
never run on an image without python3, so nobody had noticed that the minimal
package omits the `json` module. The cache probe died with
`ModuleNotFoundError: No module named 'json'`, the launcher read that as a
cache miss, and the new nvcc guard then correctly refused to build on a pod
with no compiler. The gate is now `python3 -c 'import json'` and the package is
`python3`. It cost about 2.5 minutes of pod life.

## Two operational facts that cost money when forgotten

**Creating a pod is a race — until it is not.** `LOW` means one machine and the
create is the only real test, so retry the *create* rather than re-reading
availability first. But a race can be lost outright: on 2026-11-11 the A5000
refused fourteen creates, and `get-capacity` with explicit `cudaVersions`
reported it **`Out` on 12.8, 13.0 and 13.2** in Secure EU-SE-1 while the
per-GPU read still said `LOW` — that read spans Community too. So: retry four
or five times, then run the deep probe, which is what distinguishes a window
worth waiting for from an empty pool. The same probe showed A40, A6000 and
3090 as `Low` on 13.2 only and `Out` on 12.8 and 13.0, which is why a pod asks
for `minCudaVersion: "12.8"` and lands on a 13.2 host.

**The volume decides the card, not the catalog.** A network volume is
data-centre local, so a cache in EU-SE-1 means the choice is whatever EU-SE-1
has. On 2026-11-11 that was the A40 and A6000 at $0.59 (`Low` on 13.2 only);
the A5000 and 3090 share the `AMPERE_24` pool and were `NONE` for a
continuously watched **15.7 hours** across forty-nine refused creates and thirteen
probes, on Community as well as Secure, while the 4090's Secure stock was
entirely in EU-RO-1. The A40 went `NONE` too, by 14:10Z. Both 48 GB
cards read `LOW` again by 17:47Z, but that read bought nothing. **The A6000
refused eleven creates**, and the request was stripped one axis at a time to
find out what was being refused: with and without the volume, with and without
the data-centre filter, with and without a CUDA constraint, on Community as well
as Secure. A bare pod with none of them was refused too. So it is not the
volume, the filter or the cloud tier -- there was no A6000 to rent anywhere,
while both catalog reads said `LOW`. So a schedule that must produce a proof
at a given hour cannot rest on hourly GPU capacity here, and **no read on this
API predicts a create** -- grades are advisory on 48 GB cards exactly as they
are on 24 GB ones. So **$0.59/hr is the practical floor for a cache-warm run**, and the
A5000's $0.27 is a best case that needs its pool to be free.

**And when the pool came back, it came back somewhere else.** At 02:45Z on
2026-11-12, after fifteen hours and forty-five minutes of `NONE`, the A5000 read
`LOW` on Secure -- in CA-MTL-1 and US-IL-1. EU-SE-1 was not in the
`dataCenters` array, and the create there was refused a thirty-sixth time. Two
reads four minutes apart disagreed about whether Montreal was in the list, so
even the data-centre-scoped read churns by the minute. **So the sixteen-hour
blockage was never scarcity alone -- it was scarcity intersected with one pinned
data centre.** The card is rentable; the cache is not reachable from it. Note
also what EU-SE-1 did have at that hour: the A40 at `MEDIUM`, the strongest
reading any usable card gave all window, and `HIGH` fleet-wide.

**The cache buys money, not measurement.** `gpu_prove_seconds` is measured after
the ELF exists, so a build-in-pod run on an A5000 in any data centre yields a
row directly comparable with the cached A40 row -- about seventeen minutes of
build plus two and a half minutes of proving, roughly nine cents at $0.27/hr.
The volume is a cost optimisation, not a precondition for a number. That is the
cheap way to break a data-centre pin that no amount of waiting has broken.

**The data-centre pin is documented, and so is the way out of it.** A network
volume's `dataCenter` is fixed at creation, and "only compute in the volume's
own data center can mount it, which limits GPU choice to what that data center
offers" -- which is exactly the trap above. But Runpod also has **global volumes**
(beta): region-independent storage that mounts into a Pod in any data centre, at
`/workspace`, or at `/workspace-global` when a network volume is attached too,
so one pod can hold both and copy between them. `create-global-volume` exists
and `create-pod` takes `mounts.global`.

**The whole cache cannot go there, but the part that matters can.** Two
documented limits decide it. First, "global volumes cannot set file permission
bits", so `pod-prove.sh` cannot execute the ELF in place from `$CACHE/bin`; it
would have to copy to container disk and `chmod +x`, which costs seconds.
Second and larger: `pod-run.sh` keeps `CARGO_HOME`, `RUSTUP_HOME` and the cargo
`target` tree on the cache, which is hundreds of thousands of small files, and a
global volume has neither file locking nor atomic rename -- the two things cargo
relies on. It would likely corrupt rather than merely run slowly, and at
**$0.005 per 1,000 writes and creates** a few hundred thousand files costs more
per population than the whole 10 GB network volume costs per month.

The prove path needs none of that: `key.json`, the binary and its `.sha256`, and
`artifacts/{elf,input,run-manifest.json}` -- a dozen read-only files. On a global
volume that is a few thousand **Class B** reads at $0.0005/1,000, fractions of a
cent. So the shape that works is **two caches**: the build cache stays on a
network volume where cargo is happy, and a small read-only *prove kit* goes to a
global volume so a cache-warm proof can run in whatever data centre has a card.

| | Network volume | Global volume |
| --- | --- | --- |
| Stored data | $0.07/GB/mo (<1 TB), $0.05 above, $0.14 high-performance | **not published** -- not in the docs, not on the pricing page |
| Requests | none | $0.005/1,000 writes, creates, lists; $0.0005/1,000 reads |
| Our 10 GB | $0.70/month, attached or not | would bill only what is stored, at an unquotable rate |
| At $0 balance | accrues charges | flagged, then **permanently deleted after 15 days** |

The storage half of a global volume's bill therefore cannot be quoted in
advance; it has to be read off the billing dashboard after a small test.

Seeding is the other open end. Copying across needs a pod that mounts both
volumes, so a pod in EU-SE-1 -- the thing capacity would not give us. Writing the
prove kit from a fresh build elsewhere avoids that, with
`CUDA_ARCHS=86,89,120` so it stays fat.

**The build cache holds exactly one build.** Its key covers the OS, nvcc,
rustc, `CUDA_ARCHS` and a fingerprint of the harness sources, so *any* source
edit evicts it — which is correct, and is why three small edits cost three fat
rebuilds in one day. Keying it by directory would let several coexist; that is
unbuilt.

## CPU-only, for completeness

Never measured for the full guest, and the only CPU proving datapoint — 13
shards of a synthetic program in 16.87 s on an M2 Pro — does not scale to 82
shards of 340 million cells each. Structurally: proving stops hiding behind the
executor and becomes the dominant term, memory goes from 16 GB to **35–48 GB**,
and a `cpu3c` 32 vCPU / 64 GB pod at $0.96/hr would need to finish in about
four minutes to match the GPU on cost. A realistic 20–75 minutes is
**$0.30–$1.20**, i.e. 7–25x dearer and 10–30x slower. One run with the watchdog
capped at 90 minutes would settle it for at most ~$1.40.

The one genuine attraction of a CPU pod: `PASTA_PROVE_INFLIGHT=3` *works*
there (13 shards in 10.89 s against 16.87 s sequential, 1.55x), while on CUDA
it panics — so it is where that concurrency can be studied without fighting for
a card.

## Records behind these numbers

- [mizu-gpu-core-proof-a40](optimizations/06-sp1-runtime-and-proving.md#mizu-gpu-core-proof-a40-the-first-real-gpu-proof-from-this-repository) — the first run, both arms, and the four defects it found
- [mizu-record-channel-depth](optimizations/06-sp1-runtime-and-proving.md#mizu-record-channel-depth-the-record-channel-was-the-bottleneck-not-either-side) — the −16.2% channel depth sweep
- [mizu-build-cache-volume](optimizations/07-measurement-and-tooling.md#mizu-build-cache-volume-the-volume-carries-the-build-and-a-fat-binary-is-free-at-runtime) — the volume, the fat build and the 3-minute run
- evidence: [a40](../benchmarks/gpu-core-proof-a40-sp1-6.10.json),
  [a40 repeat](../benchmarks/gpu-core-proof-a40-repeat-sp1-6.10.json),
  [a5000](../benchmarks/gpu-core-proof-a5000-sp1-6.10.json),
  [channel depth](../benchmarks/gpu-inflight-sweep-a40-sp1-6.10.json),
  [build cache](../benchmarks/gpu-build-cache-validation.json)
