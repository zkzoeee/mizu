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

## One real proof on a rented GPU: about 5 minutes, for about 5 cents

Measured end to end twice, on two different A40 hosts in EU-SE-1, both on the
101 MB `nvidia/cuda:12.8.1-base-ubuntu24.04` image with the build cache on a
network volume. The retained arm both times: 82 shards, 132,029,932 proof
bytes, 142,638,298 cycles, the same ELF and the same verification key, accepted
by the complete verifier and reloaded from disk in a fresh process.

| Segment | 2026-11-11 (`4c8i6zyte7uuo3`) | 2026-11-12 (`ukj1etk6hnwrfr`) |
| --- | ---: | ---: |
| Create → pod ready | 18 | 67 |
| Operator latency before launch¹ | 37 | — |
| Launcher: scripts, fingerprint, cache probe, tmux | 26 | 45² |
| Cache check and pre-flight | 3 | 3 |
| **Proof** | **139** | **118** |
| From-disk verification | 20 | 16 |
| Collection and terminate | 39 | 38 |
| **Pod life, as billed** | **280 (4 min 40 s)** | **317 (5 min 17 s)** |
| **Cost at $0.59/hr** | **$0.046** | **$0.052** |

**The same work on the same card type differed by 12.5%.** The prove loop
reported **110.07 s** on the first host and **96.32 s** on the second; total
proof time 138.56 s against 118.24 s, executor 128.93 s against 111.15 s, CPU
verification 7.65 s against 5.68 s. Nothing in the software differed —
identical `elf_sha256`, identical `verification_key_sha256`, identical cycle
count and shard count, the same `PASTA_R5_INFLIGHT=8` and
`PASTA_GPU_WORKERS=2`. What differed was the machine: the faster host reported
**50,899,386,368 bytes of total VRAM against 47,699,722,240**, about 3 GiB
more, so these were not equivalent A40s. **Treat any single rented-A40 timing
as carrying at least 12.5% host variance**, which is larger than most of the
per-arm deltas in the optimization records — those are measured on one host in
one session for exactly this reason, and must not be compared across pods.

The cheaper RTX A5000 at $0.27/hr proved this same arm in **128.43 s**. That is
now the *middle* of the A40 range rather than faster than it, so the earlier
claim that the A5000 beat every A40 run does not survive the second
measurement. Its price advantage is real and its speed advantage is not
established. It is also the harder card to get: it refused **thirty-six
creates** in EU-SE-1 across eighteen hours on 2026-11-11/12 and, when its pool
finally returned, returned in Montreal and Illinois instead. **Plan for the
A40's price and treat the A5000 as a windfall.**

¹ mine, not the system's, on the first run only. ² the second run paid 20 s for
a failed launch first: the cache probe compares `cuda_archs` verbatim and the
launcher defaults it to `86`, while the stored key was built for `86,89,120`,
so a warm cache read as a miss. Pass `CUDA_ARCHS` matching the stored key.
See [running a proof](docs/running-a-proof.md).

Nothing was compiled, uploaded or installed beyond `openssh-server`, `libgomp1`,
`python3`, `tmux` and certificates; no image of ours and no registry credential
were involved. [Running a proof](docs/running-a-proof.md) has the machine
choice, prices and per-phase costs, the [runbook](docs/gpu-run-on-runpod.md) has
the procedure, and the figures above come from
[the measurement](benchmarks/gpu-slim-pod-timed.json) with receipts in
`benchmarks/gpu/runs/20261111T1104Z-slim-timed`.

## Mizu development workflow

See [the cached CPU development loop](docs/development-loop.md) for targeted
native checks, standalone operation benchmarks, frozen baseline comparisons
and source-matched promotion gates. Start with `make dev-tools` and
`make dev-check`; the full matrix is a separate `make dev-gate` stage.
