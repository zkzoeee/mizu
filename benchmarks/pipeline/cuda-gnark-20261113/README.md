# CUDA Groth16: the gnark half in 37 s instead of 133 s

succinctlabs/sp1#3017 cherry-picked onto the fork's `codex/mizu-cuda-gnark`
branch, built and measured on an A40 (EU-SE-1, volume). The comparison is exact:
**the same wrap proof** (`320d4997..`, 1,340,949 bytes), the same installed
artifacts, the same card — only the gnark prover swapped.

| | CPU gnark | CUDA gnark |
| --- | ---: | ---: |
| `--mode from-wrap` | 133 s (130.9 s in gnark) | **37 s (35.6 s in gnark)** |
| peak VRAM | — | **3,141 MiB** |
| public inputs | identical | identical |
| `groth16_vkey_hash` | `4388a21c..` | `4388a21c..` |
| proof bytes | differ (gnark proving is randomised) | |

**-72% on the half that #3017 targets.** Both proofs verify in the contract:
`contracts-check` has 13 tests, including the CUDA-proved one at 225,954 gas.

## The two risks, both cleared

**It works through the in-process node.** The PR's caveat points users at
`SP1_PROVER=cuda` with `sp1-gpu-server`, which this project does not use — the
`cuda` feature on `sp1-recursion-gnark-ffi` is enough, routed through
`sp1-prover`'s new `gnark-cuda` passthrough so no driver needs a direct
dependency on the FFI crate.

**VRAM is a non-issue: 3,141 MiB.** The PR reports ~10.3 GiB for its Groth16
prover, but that is the whole `sp1-gpu-server` including the STARK pool. Here
the gnark step runs alone, so the "does the STARK pool get released" question
never arises for `--mode from-wrap` — and 3 GiB leaves room on any card we use.

## Build

9m46s on a warm volume: nvcc compiles three kernels, the Go library builds with
the `cuda` tag, ICICLE is gone from `go.mod`. `CUDA_ARCHS` now drives
`-arch=sm_NN` instead of the PR's `-arch=all-major`, which would have compiled
every major architecture for code that runs on one known card.

```sh
cargo build --release -p pipeline-runner --bin pipeline-runner --features gnark-cuda
CUDA_ARCHS=86   # A40/A5000/A6000; 89 for a 4090
```

## What this does and does not speed up

It touches **only** the gnark half. `--mode wrap` — execution, core proving,
the reduce tree, shrink and wrap — contains no gnark and is unchanged at ~205 s.
A full Groth16 proof goes from ~325 s to roughly ~230 s; the split pipeline goes
from 205 + 133 to 205 + 37.

## Still unmerged

The branch has no upstream review and its kernel tests are explicitly not in the
repository. Every proof is verified before it is returned and again in the
contract, so a wrong proof fails loudly rather than silently — but this is
carried code, and it stays on its own branch until #3017 merges.
