# Mizu — Pickles verification on SP1

Mizu consolidates verifier and Pasta-optimization iterations on the official
[o1js-to-zkvm](https://github.com/o1-labs/o1js-to-zkvm) repository. Its guest/host
paths and boolean-output interface follow upstream. SP1 is pinned to **6.10.0**.

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

The fresh retained core baseline is **142,660,047 cycles**, with boolean `true`
output. See [validation](docs/validation.md) and the [measurement](benchmarks/core-sp1-6.10.json).

This builds and executes the core guest with `benchmarks/retained.json`, pins
source/ELF/fixture identity, and records fresh cycles. Compatibility optimization
variable names are preserved to keep the implementation stable. The receipt
example's historical 142,692,323-cycle result is recorded separately in
`benchmarks/reference`; its 577-byte ABI differs from the core boolean guest.

Read [the module map](docs/modules.md), [upstream sync guide](dependencies/README.md),
the [dependency and optimization inventory](docs/dependencies-and-optimizations.md),
[changed-file list](docs/file-changes.md), and [receipt example](examples/settlement/README.md). Full historical runs and
retired applications are recoverable from the external consolidation archive.
