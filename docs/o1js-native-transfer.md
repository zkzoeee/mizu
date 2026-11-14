# Does Mizu's work transfer to o1js? The MSM answer is no

Measured 2026-11-14, after establishing that o1js's prover is the same Rust we
optimize. Evidence: `benchmarks/native/msm-o1js-transfer-20261114/`, harness
`crates/pickles-verifier/tests/pallas_msm_native_timing.rs`.

## Why the question is reasonable

o1js is TypeScript over **OCaml** (Snarky's constraint DSL and Pickles'
recursion, compiled to JS by js_of_ocaml) over **Rust** (kimchi, mina-curves,
mina-poseidon, poly-commitment). The heavy arithmetic was never OCaml. Those
Rust crates reach o1js through two thin bindings — `kimchi-wasm` in browsers and
`kimchi-napi` natively on Node since o1js 2.15.0, where `kimchi-napi` depends on
`kimchi_wasm` and adds a Node-API surface. Both are workspace members of our own
vendored `proof-systems`.

So a change in those crates reaches o1js on both paths with no binding work, and
our fork changes exactly those crates. The transfer question is therefore not
"can we" but "would it help".

## The measurement

Pallas MSM, best of 5 (3 at 2^16), every arm asserted equal to arkworks'
`msm_bigint` so a timing gap cannot come from one arm computing less.

| threads | size | arkworks | ours, affine | ours, affine+GLV | ours vs arkworks |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 2^12 | 32.90 ms | 65.25 ms | 55.99 ms | **1.70x slower** |
| 1 | 2^14 | 109.89 ms | 204.08 ms | 187.89 ms | **1.71x slower** |
| 1 | 2^16 | 371.70 ms | 684.42 ms | 642.17 ms | **1.73x slower** |
| 10 | 2^12 | 6.56 ms | 65.68 ms | 55.97 ms | **8.5x slower** |
| 10 | 2^14 | 18.27 ms | 204.40 ms | 187.98 ms | **10.3x slower** |
| 10 | 2^16 | 61.51 ms | 691.11 ms | 635.15 ms | **10.3x slower** |

## Three findings

**GLV does work — within our own path.** It is 5.9% to 14.2% faster than our
affine schedule, and the gain *shrinks* as the MSM grows, which is the wrong
direction for o1js's 2^15-2^16 circuits.

**Our whole canonical path is slower than stock arkworks**, by 1.7x even
single-threaded. The canonical coordinate conversion exists to feed SP1's
fixed-modulus syscalls; with no syscall behind it, the conversion is pure
overhead.

**And it does not parallelise.** Our times are identical at 1 and 10 threads,
while arkworks scales about 4.5x at 2^16. That is not an oversight: a zkVM guest
is single-threaded, so parallelism was worth nothing where this code was built
and everything where o1js runs.

## What this means

**Do not port the MSM schedules to o1js.** Dropping them in would cost roughly
an order of magnitude on the dominant cost of a kimchi proof. GLV is a real
algorithmic win, but it is a 6-14% win on a path 10x behind the baseline — the
only honest way to ship it to o1js is as a patch to *arkworks'* parallel
Pippenger, not as our schedule.

This is the second time the zkVM cost model has misled a transfer estimate.
Mizu's figures were taken where a Pasta add is ~98 cycle-equivalents, memory
interaction is ~43% of trace cells, and there are no threads. None of those hold
natively. The other candidates named in
[the flag reference](build-flags.md) — the IPA allocation work, precomputed
setup and VK digest — are untested natively and should be measured the same way
before anyone argues for them.

## Reproducing

```sh
RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo test --profile native-test --locked \
  -p pickles-verifier --test pallas_msm_native_timing -- --ignored --nocapture
```

Two caveats on the harness. `native-test` is release with `lto=false` and
`codegen-units=4`, so neither arm is maximally optimized; and the machine is a
shared laptop, which is why arkworks' 2^16 figure ranged 61.5-82.4 ms across
runs while ours held within 0.3%. Neither affects a 10x conclusion.
