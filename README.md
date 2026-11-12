# Mizu — Pickles verification on SP1

Mizu consolidates verifier and Pasta-optimization iterations on the official
[o1js-to-zkvm](https://github.com/o1-labs/o1js-to-zkvm) repository. Its guest/host
paths and boolean-output interface follow upstream. SP1 is pinned to **6.10.0**;
proof-systems uses upstream **0.7.0 / `b1d9829`** plus our four optimization commits.

| Area | Location | Responsibility |
| --- | --- | --- |
| Pickles verification | `crates/pickles-verifier` | Wire validation, recursive checks, optimized MSM/SRS setup |
| SP1 guest | `crates/o1-verifier` | Verify one proof; commit boolean true; abort on invalid proof |
| Host CLI | `crates/o1-verifier-host` | Upstream `o1zkvm` execute/prove interface |
| CPU measurements | `crates/cycle-runner`, `scripts/benchmark.py` | Bounded execution, corruption controls, reproducible cycles |
| Receipt example | `examples/settlement` | Optional versioned receipt types and guest, separate workspace |
| Pasta validation | `tools/pasta-proof`, `tools/pasta-guest` | Focused AIR/ABI harnesses, no run dumps |
| Dependency forks | `vendor/algebra`, `vendor/proof-systems`, `vendor/sp1` | Pinned upstream-based patch stacks |

## Setup and checks

```sh
python3 dependencies/bootstrap.py
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 cargo test --profile native-test --locked -p pickles-verifier -p cycle-runner -- --test-threads=1
cargo check --profile native-test --locked -p pickles-verifier --no-default-features
```

Bootstrap restores the local fork commits from upstream bases plus bundled patch
stacks. Use it after a normal clone; the fork commits are not published yet.
`--offline` works when the upstream base objects are already available locally.
The original optional `mina` submodule is only needed by upstream's OCaml fixture
export commands; the verifier uses the directly pinned proof-systems fork.

Install SP1/protoc with `make install` if needed. `make build-rust` builds the
upstream-style host and guest. Real proving modes require an appropriately sized
host and explicit authorization; ordinary compilation and native tests do not
produce proofs.

## Retained optimized benchmark

```sh
python3 scripts/benchmark.py run --output benchmarks/runs/first
```

The current retained core benchmark is **142,650,295 cycles**, with boolean `true`
output. This is 9,752 fewer cycles than the previous optimized proof-systems 0.4.1
baseline. See [upgrade validation](docs/proof-systems-upgrade.md) and the
[measurement](benchmarks/core-proof-systems-0.7-sp1-6.10.json).

This builds and executes the core guest with `benchmarks/retained.json`, pins
source/ELF/fixture identity, and records fresh cycles. Compatibility optimization
variable names are preserved to keep the implementation stable. The receipt
example's historical 142,692,323-cycle result is recorded separately in
`benchmarks/reference`; its 577-byte ABI differs from the core boolean guest.

Read [the module map](docs/modules.md), [upstream sync guide](dependencies/README.md),
the [dependency and optimization inventory](docs/dependencies-and-optimizations.md),
[optimization work records by area](docs/optimizations/README.md),
[the fuzzing reference](docs/fuzzing.md),
[running a proof: machines, time and price](docs/running-a-proof.md),
[the rented-GPU runbook](docs/gpu-run-on-runpod.md),
[changed-file list](docs/file-changes.md), and [receipt example](examples/settlement/README.md). Full historical runs and
retired applications are recoverable from the external consolidation archive.

## One real proof on a rented GPU: under two minutes of proving, two to eight cents of pod

Eleven measurements of the same retained arm — 82 shards, 132,029,932 proof bytes,
142,638,298 cycles, the same ELF and the same verification key, accepted by the
complete verifier and reloaded from disk in a fresh process — at the production
`PASTA_R5_INFLIGHT=8`, on nine distinct physical cards in EU-SE-1. *Prove cost*
is the prove loop at the card's hourly rate; *pod billed* is what Runpod
actually charged for that rental, GPU plus disk, from the billing API.

| Card | Host | $/hr | Prove (s) | Total (s) | Prove cost | Pod billed | Run |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| A40 | `7d6ed7f9` | 0.59 | **96.32** | 118.24 | $0.0158 | $0.0530 | `20261112T0502Z-a40-cachewarm` |
| A6000 | `0e836af8` | 0.59 | 100.79 | 123.26 | $0.0165 | ~$0.055ᶜ | `20261112T0810Z-a6000` |
| A40 | `508a1a8a` | 0.59 | 107.74 | 134.16 | $0.0177 | —ᵃ | `20261110T1655Z-inflight-sweep` |
| A40 | `9686af90` | 0.59 | 108.36 | 134.44 | $0.0178 | $0.0596 | `20261111T0910Z-cudaprobe-validation` |
| A40 | `37cd5cd6` | 0.59 | 108.89 | 136.14 | $0.0178 | $0.2014ᵇ | `20261111T0628Z-fatbuild-populate` |
| A40 | `508a1a8a` | 0.59 | 109.41 | 135.18 | $0.0179 | —ᵃ | `20261110T1655Z-inflight-sweep` |
| A40 | `d6ffcd7e` | 0.59 | 110.07 | 138.56 | $0.0180 | $0.0452 | `20261111T1104Z-slim-timed` |
| A40 | `37cd5cd6` | 0.59 | 111.36 | 137.98 | $0.0183 | $0.0759 | `20261111T0649Z-cachehit` |
| A5000 | `b2d5ccd2` | 0.27 | 104.09 | 128.43 | $0.0078 | $0.0821ᵇ | `20261111T0751Z-a5000` |
| A5000 | `a41332a8` | 0.27 | 106.88 | 131.92 | $0.0080 | $0.0328 | `20261111T0856Z-a5000-proveonly` |
| A5000 | `0d82daa9` | 0.27 | 108.23 | 135.54 | **$0.0081** | **$0.0225** | `20261111T1041Z-slim-base-image` |

ᵃ one pod proved six times across the inflight sweep; $0.4238 for the rental as
a whole is not a per-proof figure. ᵇ this pod also compiled the CUDA prover, so
its bill is a build, not a proof. ᶜ arithmetic from 337 s of pod life: Runpod
had not posted this pod's bill when this was written. The five cache-warm single-proof rentals
**cost $0.0225 to $0.0759 end to end**, create through terminate; the cheapest
complete verified proof on record is **2.3 cents**, on the A5000 under the
101 MB `nvidia/cuda:12.8.1-base-ubuntu24.04` image with the build cache on a
network volume. All eleven rentals in the whole measurement campaign, builds
included, billed **$1.62** against the authorized $2.00 cap.

**A40 hosts are reproducible to 3.4%, and one host in seven was not an A40 like
the others.** Six of the seven A40 measurements, spanning four cards and two
repeat pairs on the same card, fall in **107.74–111.36 s** — a 3.4% band. The
seventh (`7d6ed7f9`, 2026-11-12) proved in **96.32 s**, **11.8% below that
cluster's median** and 10.6% below even its fastest member, with nothing in the
software different: identical `elf_sha256`, identical
`verification_key_sha256`, identical cycle and shard counts, the same
`PASTA_R5_INFLIGHT=8` and `PASTA_GPU_WORKERS=2`. What differed was the card.
The fast host reported **50,899,386,368 bytes of total VRAM against the other
six hosts' 47,699,722,240**, and those are the only two values in the record —
not a continuum. Against the A40's nominal 48 GiB the fast host is **1.2%
short** and the other six are **7.5% short**; for comparison, all three A5000s
report an identical 25,285,623,808 bytes, **1.9% short** of their nominal
24 GiB. The extra ~6.3% on the six is the size of ECC's reservation (~6.25%),
and ECC costs memory bandwidth as well as capacity.

**The A6000 run settled half of that.** It is the first card to report its own
ECC state, and it reports `Enabled` — together with 46,068 MiB and
47,699,722,240 bytes, byte for byte what the six A40s report, 6.27% below the
nominal 49,152 MiB. So **the shortfall is the ECC reservation**, and the fast
A40's 49,140 MiB — 12 MiB below nominal — means that card had ECC off. The
inference no longer rests on arithmetic alone. What the A6000 does **not**
explain is the ordering: an ECC-*on* card at 100.79 s is still slower than the
ECC-*off* A40 at 96.32 s, despite 10.3% more nominal bandwidth. So ECC-off buys
more than its capacity figure suggests, or that host differs in some further
way. Settling it needs an ECC-on and an ECC-off pod of the *same* model, and
every run from now on records the state to make that possible.
Either way, **a single rented-A40 timing cannot be compared against one from
another pod** — not because any one card is noisy, but because the pool is not
uniform. The per-arm deltas in the optimization records are measured on one host
in one session for exactly this reason.

**The A5000 is both faster and less than half the price.** Its three runs, on
three cards, give **104.09–108.23 s, median 106.88 s — 2.1% faster than the A40
cluster's 109.15 s median**, and faster than every A40 except the ECC outlier.
At $0.27/hr against $0.59 that is **$0.0080 of prove against $0.0179, 55%
cheaper per proof**. An earlier revision of this section retired the A5000's
speed advantage on the strength of a single A40 run; the full set restores it,
modestly and consistently. What the A5000 does not have is availability: it
refused **thirty-six creates** in EU-SE-1 across eighteen hours on
2026-11-11/12 and, when its pool finally returned, returned in Montreal and
Illinois instead. **Budget at the A40's price and treat the A5000 as a
windfall.**

**The A6000 is the fastest card measured that is actually obtainable, and the
cleanest comparison in the set.** At **100.79 s** it is 7.7% under the A40
cluster's median and 5.7% under the A5000's, beaten only by the ECC-off A40.
Against the A40 cluster it is as controlled as a rented comparison gets: the
same GA102 family and SM count, the same 46,068 MiB with ECC on, the same Xeon
Gold 6342, the same 96 vCPUs, the same driver 595.91.07 — and nominally 768
GB/s of bandwidth against the A40's 696, **+10.3% of bandwidth for −7.7% of
prove time**. Device-busy was 66.1 s (A40 68.6, A5000 63.0) and peak VRAM
18,991 MiB, so it needs the 48 GB no more than the A40 did. Two caveats: it is
**n=1**, and it is still a cross-pod comparison — now with the confound I
identified controlled, but not with every confound controlled. Its executor
also ran 116.01 s against the A40's 127.01 on an identical CPU model, which the
card cannot explain, so some of the 7.7% is the same unattributed host
difference seen before. **At $0.59/hr it buys speed, not economy**: $0.0165 a
proof against the A5000's $0.0080. It took four creates.

Where the five minutes goes, from the two runs timed end to end:

| Segment | 2026-11-11 (`4c8i6zyte7uuo3`) | 2026-11-12 (`ukj1etk6hnwrfr`) |
| --- | ---: | ---: |
| Create → pod ready | 18 | 67 |
| Operator latency before launch¹ | 37 | — |
| Launcher: scripts, fingerprint, cache probe, tmux | 26 | 45² |
| Cache check and pre-flight | 3 | 3 |
| **Proof** | **139** | **118** |
| From-disk verification | 20 | 16 |
| Collection and terminate | 39 | 38 |
| **Pod life** | **280 (4 min 40 s)** | **317 (5 min 17 s)** |
| **Billed** | **$0.0452** | **$0.0530** |

So **the proof is 37% to 50% of a cache-warm rental**; the rest is create,
launch, verification and teardown, and the largest single variable is how long
Runpod takes to hand over the pod — 18 s on one run, 67 s on the other.

¹ mine, not the system's, on the first run only. ² the second run paid 20 s for
a failed launch first: the cache probe compared `cuda_archs` verbatim while the
launcher defaulted it to `86`, so a cache built for `86,89,120` read as a miss.
Fixed since: the probe reads the card's compute capability over SSH and a stored
build counts as a hit when its arch list *covers* the card.
See [running a proof](docs/running-a-proof.md).

Nothing was compiled, uploaded or installed beyond `openssh-server`, `libgomp1`,
`python3`, `tmux` and certificates; no image of ours and no registry credential
were involved. [Running a proof](docs/running-a-proof.md) has the machine
choice, prices and per-phase costs, the [runbook](docs/gpu-run-on-runpod.md) has
the procedure, and the figures above come from
[every recorded measurement with its cost](benchmarks/gpu/cross-machine-timings.json)
and [the end-to-end timing](benchmarks/gpu-slim-pod-timed.json), with receipts
in `benchmarks/gpu/runs/`.

## Mizu development workflow

See [the cached CPU development loop](docs/development-loop.md) for targeted
native checks, standalone operation benchmarks, frozen baseline comparisons
and source-matched promotion gates. Start with `make dev-tools` and
`make dev-check`; the full matrix is a separate `make dev-gate` stage.
