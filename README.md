# Mizu — Mina proofs in SP1, fast enough to settle on Ethereum

Mizu verifies a real Mina blockchain SNARK (a Pickles/kimchi recursive proof)
inside the SP1 zkVM, wraps the result in Groth16, and checks it in a Solidity
contract. It is built on the official
[o1js-to-zkvm](https://github.com/o1-labs/o1js-to-zkvm) repository and keeps its
guest/host paths and boolean-output interface.

## Three results

**1. About 33x fewer cycles.** Verifying the pinned mainnet fixture costs
**130,367,324** SP1 cycles with Mizu's retained configuration, against roughly
**4.3 billion** for the same verifier unoptimized.

**2. The whole proof flow works, and ends on Ethereum.** Core → compress →
shrink → wrap → **Groth16**, a **324-byte** proof, verified in a Solidity
contract at **~226k gas**. No trusted setup of our own was needed: the fork's
wrap verifying key is byte-identical to upstream's, so Succinct's released
artifacts apply.

**3. Mina state is usable on an EVM.** An optional **keccak mirror tree** walks
the same ledger diff in both Poseidon and keccak, so a contract can prove
inclusion of any account against a keccak root it can recompute cheaply.

## The numbers

| | measured |
| --- | ---: |
| Verifier cycles, retained configuration | **130,367,324** |
| Shards | 84 |
| Core proof, prove loop | **96–111 s** |
| Cheapest complete verified core proof, end to end | **$0.023** |
| Full Groth16 proof, 1xA40 | **~325 s**, ~$0.053 |
| …with the CUDA gnark prover | **~230 s**, ~$0.038 |
| Groth16 proof size | 324 bytes |
| Solidity verification | 225,954 gas |
| Mirror-tree proof, end to end | 71 s |

Prices are the card's hourly rate (A40 at $0.59/hr) against measured wall time.
Everything here is a recorded run, not a projection —
[running a proof](docs/running-a-proof.md) has the per-card and per-phase
breakdown across eleven measurements on nine physical cards, and
[the Groth16 runs](docs/groth16-runs.md) decompose the 325 s stage by stage.

### Where the 33x comes from

| step | cycles | |
| --- | ---: | --- |
| the verifier unoptimized | ~4,340,000,000 | see the note below |
| Mizu's retained configuration (40 settings) | 142,650,295 | ~30x fewer |
| + the two Pasta precompiles | **130,367,324** | **-8.60%** |
| + `mizu/dev` compact `w=16` (no new AIRs) | **84,985,706** | **-34.81%** vs 130.37M; verified core proof: **67 shards**, **117.09 s** wall (A40) / **63.80 s** wall (RTX 4090) |

**On the ~4.3 billion.** That is not a measurement made in this repository under
this harness. It is the same verifier's cost with the optimizations off, recorded
as an execution baseline at **4,343,433,917** cycles in Miza's validation of the
restored IPA implementation on stock SP1 6.10.0, and independently corroborated
by OpenVM's published SP1 reference for this workload at **4,378,867,074** —
0.8% apart. The matched, same-harness comparison inside this repository is the
last two rows: 142,650,295 against 130,367,324. Treat the 33x as the honest
order of magnitude and the 8.60% as the audited figure.
[o1-openvm-review.md](docs/optimizations/o1-openvm-review.md) explains why a bare
quotient across VMs is not a matched comparison.

## Layout

| Area | Location | Responsibility |
| --- | --- | --- |
| Pickles verification | `crates/pickles-verifier` | Wire validation, recursive checks, optimized MSM/SRS setup |
| SP1 guest | `crates/o1-verifier` | Verify one proof; commit boolean true; abort on invalid proof |
| Host CLI | `crates/o1-verifier-host` | Upstream `o1zkvm` execute/prove interface |
| Proof pipeline | `crates/pipeline-runner`, `tools/pipeline-cuda` | Core → wrap → Groth16, VK maps, shape audits |
| EVM verification | `contracts-check` | Groth16 verifier contracts and their tests |
| Mirror tree | `crates/poseidon-bench` | Dual Poseidon/keccak ledger walk |
| CPU measurements | `crates/cycle-runner`, `scripts/benchmark.py` | Bounded execution, corruption controls, reproducible cycles |
| Receipt example | `examples/settlement` | Optional versioned receipt types and guest, separate workspace |
| Pasta validation | `tools/pasta-proof`, `tools/pasta-guest` | Focused AIR/ABI harnesses |
| Dependency forks | `vendor/algebra`, `vendor/proof-systems`, `vendor/sp1` | Pinned upstream-based patch stacks |

SP1 is pinned to **6.10.0**; proof-systems is upstream **0.7.0 / `b1d9829`** plus
four optimization commits.

## Setup and checks

```sh
python3 dependencies/bootstrap.py
```

```sh
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 cargo test --profile native-test --locked -p pickles-verifier -p cycle-runner -- --test-threads=1
```

Bootstrap restores the local fork commits from upstream bases plus bundled patch
stacks; use it after a normal clone, since the fork commits are not published
yet. `--offline` works when the upstream base objects are already local. The
optional `mina` submodule is only needed by upstream's OCaml fixture-export
commands.

`make install` installs SP1 and protoc. **Use `make build-guest`, not
`make build-rust`, to build a guest you intend to prove with** — it applies a
recipe and writes a provenance receipt; `build-rust` builds the upstream-style
host and guest with default settings. Real proving needs an appropriately sized
host and explicit authorization; ordinary compilation and native tests produce
no proofs.

Reproduce the retained benchmark with:

```sh
python3 scripts/benchmark.py run --output benchmarks/runs/first
```

## Documentation

**Results and cost**
- [Running a proof](docs/running-a-proof.md) — machines, wall time, prices
- [Groth16 runs](docs/groth16-runs.md) — the measured stage-by-stage breakdown
- [Rented-GPU runbook](docs/gpu-run-on-runpod.md) — the procedure

**How it is built**
- [Build flags](docs/build-flags.md) — all 54 that shape the guest, one row each
- [Module map](docs/modules.md) and [changed-file list](docs/file-changes.md)
- [Dependency and optimization inventory](docs/dependencies-and-optimizations.md)
- [Optimization work records by area](docs/optimizations/README.md)
- [Upstream sync guide](dependencies/README.md)

**Design and plans**
- [Diff-proof plan](docs/diff-proof-plan.md) — the mirror tree and where its
  root should be produced
- [Proof pipeline plan](docs/proof-pipeline-plan.md)

**Working on it**
- [Cached CPU development loop](docs/development-loop.md) — start with
  `make dev-tools` and `make dev-check`
- [Fuzzing reference](docs/fuzzing.md) and [validation](docs/validation.md)
