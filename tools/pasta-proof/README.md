# Focused Pasta validation harness (SP1 6.10)

This standalone CPU/CUDA-capable harness contains the retained AIR/record
analysis, manifest gates and adversarial verification checks. It pins the four
Pasta crates from `vendor/sp1` in their native upstream paths. CPU unit tests and
compilation do not generate proofs:

```sh
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 cargo test --release --locked --manifest-path tools/pasta-proof/Cargo.toml --lib --bin pasta-precompile-proof-6-10 -- --test-threads=1
```

The default executable mode is registration. Real proof modes are opt-in and
require `PASTA_ALLOW_REAL_PROOF=1`; do not enable that gate or CUDA on the shared
Mac. Micro-guests live in `tools/pasta-guest`. Export
raw fixture input with the `export-fixture` binary in `crates/cycle-runner`.
All prior per-run dumps are retained in the external consolidation archive.
