# Provenance for the guest ELF `dc8e4b40..`

The ELF every Groth16 proof in `benchmarks/pipeline/` was made against, with
the receipt `scripts/benchmark.py` writes: `elf_sha256`, the builder commit,
both rustc versions, and a hash of every source file that went into it.

This was missing, and it cost an afternoon on 2026-11-13: with no receipt, the
only way to find out how the ELF had been built was to rebuild it four
different ways and compare.

## The recipe

```sh
make build-guest                     # GUEST_CONFIG=benchmarks/dev/pasta-poseidon.json
# = python3 scripts/benchmark.py build --config ... --output ...
# = cargo prove build --locked --rustflags=-C,passes=lower-atomic \
#       --warning-level minimal --packages o1-verifier --binaries o1-verifier
#   with ambient ZEKO_*/MIZU_* scrubbed and the config's optimization_env applied
```

| | |
| --- | --- |
| `elf_sha256` | `dc8e4b406c7f075cc0c5d2e3dab11eb7bcd9083a15aa9e4934d8a6ddacd37b51` |
| builder | `cargo-prove sp1 (d83ed91 2026-10-23)` |
| host rustc | 1.92.0 (ded5c06cf 2025-12-08) |
| guest rustc | 1.96.0-dev (`+succinct`) |
| rebuilt at | revision `4ff59ed`, 2026-11-13 |

## It is reproducible, and `make build-rust` is not the recipe

Rebuilt on 2026-11-13 from the committed tree, the harness produced
`dc8e4b40..` **byte for byte** — an ELF first built on 2026-11-12. That is the
property the on-chain registry needs: `programVKey` derives from this hash, so
a deployed entry stays valid as long as the recipe does.

What does *not* reproduce it:

| path | ELF |
| --- | --- |
| `scripts/benchmark.py build` (the harness) | **`dc8e4b40..`** |
| `make build-rust` (`sp1_build` with `Default::default()`) | `d4ed009f..` |
| `make build-rust` with the pre-2026-11-13 build scripts | `d4ed009f..` |

The third row rules out the build-script edit of 2026-11-13: changing a build
script does **not** move the guest ELF. The difference is purely the recipe —
`--locked`, the explicit rustflags and warning level, and the scrubbed
environment.
