# from-wrap amortisation, measured on both gnark backends

The Go prover keeps the Groth16 circuit and its proving key in package globals
for the life of the process — `globalR1cs`/`globalPk` behind `globalMutex`
(`prove_groth16.go:19-23`) on the CPU path, and a `groth16CudaProvers` map keyed
by `dataDir` on the CUDA path, whose own comment says it "keeps the circuit and
its proving key in host memory for the life of the process". Nothing used that:
`--mode from-wrap` took one `--wrap-in` and exited.

`--wrap-in` is now repeatable. **This is the measurement that it works.**

One pod, one A40 (EU-SE-1, 96 cores, 46,068 MiB, ECC **Enabled**, compute 8.6),
$0.59/hr. Both arms prove **the same wrap proof three times in one process** —
`320d4997..`, 1,340,949 bytes, the same input the 133 s → 37 s CUDA comparison
used — so any difference between proof 1 and proofs 2-3 is the cached state and
nothing else. All six proofs returned identical public inputs and the same
`groth16_vkey_hash 4388a21c..`, each verified inside `groth16_from_wrap` before
it returned.

| | proof 1 | proof 2 | proof 3 | rest mean | **saved** | as % of proof 1 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| CPU gnark | 139.5 s | 80.0 s | 78.7 s | 79.4 s | **60.1 s** | **43%** |
| CUDA gnark | 39.9 s | 13.1 s | 11.0 s | 12.0 s | **27.9 s** | **70%** |

Three proofs took 298.2 s on CPU and 64.0 s on CUDA.

## The evidence is direct, not inferred

gnark prints its own stage timings, and on the CPU arm they appear **once**:

```
wrap proof 1/3:  Reading R1CS took 32.854736809s
                 Reading proving key took 18.482908199s
                 Generating proof took 1m27.096549847s      -> 139.5s
wrap proof 2/3:  (no Reading lines at all)
                 Generating proof took 1m19.215293032s      ->  80.0s
wrap proof 3/3:  (no Reading lines at all)
                 Generating proof took 1m17.741254805s      ->  78.7s
```

## Two things the measurement corrected

**The CUDA saving is 27.9 s, not the ~10-13 s this project estimated.** That
estimate reasoned that #3017 skips the R1CS debug info and the key's twiddle
tables and reads both in parallel, so the cacheable part had to be a small slice
of the 35.6 s gnark half. Wrong by more than double: **70% of the CUDA gnark
half is per-process setup**, and the actual per-proof proving is 12 s. The
estimate was bounded correctly and apportioned badly.

**The saving exceeds the artifact reads.** On CPU the two reads are 51.3 s but
60.1 s is saved, because "Generating proof" itself ran 87.1 s on proof 1 against
79.2 s and 77.7 s after — about 9 s of first-call warm-up on top of the reads.
So this is not purely an I/O effect and the figure cannot be derived from the
artifact sizes.

## What it means for the pipeline

After CUDA gnark a single proof is roughly 205 s of wrap plus 37 s of gnark.
Batched, proofs after the first cost 205 + 12 s, so **the gnark half falls to
about 5% of the proof** and the wrap half is the whole remaining target. Nothing
in the driver reaches it: it is execution, core proving over 84 shards, the
reduce tree, shrink and wrap.

Two further per-process costs are **not** addressed here, because from-wrap
builds no node and pays neither: `node_build` at 17.9 s and `setup` at 7.3 s in
run C. Amortising those needs a multi-proof *full* pipeline holding the node
open across several ELF/input pairs.

## Caveats

- **These are this-pod figures.** This A40's artifact reads ran slower than run
  C's (32.9 + 18.5 s against 24.0 + 16.0 s), so the CPU first-proof cost is not
  a correction to run C's 130.1 s gnark half.
- Three proofs is enough to separate first from subsequent and no more; proofs 2
  and 3 differ by 1.3 s on CPU and 2.1 s on CUDA.
- Same input every time. Distinct wrap proofs would exercise witness generation
  differently, though the cached state is input-independent.
- The CUDA arm ran from the fork's `codex/mizu-cuda-gnark` files staged onto the
  pod as a tarball, since the pod's source is not a git checkout. `build_exit=0`,
  `libsp1gpu.a` linked, `go.mod` free of icicle.

## Reproducing

`amortise.sh` builds and runs an arm; `N` sets the proof count and `FEATURES`
picks the backend (`native-gnark` or `gnark-cuda`). It needs the populated
volume, a wrap proof at `/workspace/out/wrap.bin`, and `SP1_CIRCUIT_MODE` unset
so the released artifacts are used rather than locally built dev keys.

Note for the runbook: the pod image carries no rust, go or protoc. `bootstrap.log`
is the 29 s that installs rustc 1.92.0, protobuf-compiler and libclang-18, then
symlinks the volume's Go, cargo registry and SP1 artifact directories.
