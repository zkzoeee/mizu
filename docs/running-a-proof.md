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
of `12.8`. Then terminate it: a stopped pod still bills its disk.

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
| RTX 4090 | 24 GB | sm_89 | $0.89 | $0.34 | **MEDIUM**, 6 DCs | best placement odds; community needs a baked image |
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
| Slim base image + start command (unmeasured) | ~4 min | ~$0.018 | ~$0.039 |
| Baked image, binary inside, no volume (unmeasured) | ~3.5 min | ~$0.016 | ~$0.034 |

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

### What is still unmeasured, and the two ways it could fail

None of this has been run. One ~$0.05 pod settles it, and the start command
prints both answers before anything expensive:

1. **`ldd` on the cached binary** — any `not found` names a library the base
   image lacks. The reasoning above says there should be none; the reasoning is
   not a run.
2. **Driver injection into a `base` image** — `nvidia-smi -L` must answer. It
   does in every CUDA container; it has not been checked here.

And one thing that could make the win smaller than the sizes suggest: RunPod
caches images per machine, and their docs put a first pull at *"20 seconds to 5
minutes"* while an image "used often" starts very fast. `runpod/pytorch` is
about the most-used image on the platform, so our measured ~110 s is not a
guaranteed worst case, and a cold 101 MB image beats a cold 10 GB one by far
more than it beats a warm one.

## Two operational facts that cost money when forgotten

**Creating a pod is a race, not a query.** `LOW` means one machine, and the
create is the only real test. Across this work, nine creates were refused in
one sitting before one landed, and the A5000 took three attempts. Retry the
*create*; re-reading availability first only wastes the window.

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
