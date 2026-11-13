# Groth16 runs: measured breakdowns

Three Groth16 proofs exist, all on a RunPod **A40** ($0.59/hr, ECC on, 46,068 MiB,
sm_86), all verified, all under **Succinct's released v6.1.0 artifacts** with no
ceremony of our own. Records in `benchmarks/pipeline/groth16-20261112/` and
`benchmarks/pipeline/groth16-poseidon-retained-20261113/`.

| | run A | run B | run C |
| --- | --- | --- | --- |
| date | 2026-11-12 | 2026-11-13 | 2026-11-13 |
| guest | default ELF | Poseidon+Fp precompiles | same |
| `elf_sha256` | `0cfb3eaf..` | `dc8e4b40..` | `dc8e4b40..` |
| cycles | 142,638,298 | **130,367,324** | 130,367,324 |
| Pasta retention | no | **yes** | yes |
| shards | 138 | **84** | 84 |
| VK map | 67 keys | 33 keys | 33 keys |
| network volume | no | yes | yes, fully warm |
| **wall clock** | 9m09.5s | 10m04.9s | **5m50.9s** |
| `prove` | 497.4 s | 547.5 s | **294.5 s** |
| raw proof | 324 B | 324 B | 324 B |

Run C is the steady state: a fresh pod, nothing uploaded, nothing built, nothing
downloaded.

## Where run C's 5m50.9s goes

Timestamps from `/workspace/out/run2/run.log`; the `seconds` block is the
driver's own instrumentation.

| stage | time | note |
| --- | ---: | --- |
| `node_build` | 17.9 s | CUDA context, prover setup |
| `execute` | 26.0 s | guest run for the cycle count |
| `setup` | 7.3 s | program key |
| **`prove`** | **294.5 s** | broken down below |
| `verify` | 0.4 s | SP1 verify of the Groth16 proof |

And inside `prove`:

| sub-stage | time | boundary |
| --- | ---: | --- |
| re-execution | ~45 s | `prove_with_mode` re-executes from the ELF; 00:53:32→00:54:17 |
| STARK core → compress → shrink → wrap | **~111 s** | 00:54:17→00:56:08 |
| artifact check | 0 s | *"already installed"* — the cached path |
| gnark read R1CS | 24.0 s | 2.4 GB, deserialisation not I/O |
| gnark read proving key | **16.0 s** | 5.9 GB from the volume; 3.8-4.0 s from container disk |
| gnark witness | 1.2 s | |
| gnark solver | 25.4 s | 15,972,262 constraints |
| gnark prover | **63.5 s** | the only part ICICLE would accelerate |
| gnark verify | 3.2 ms | the pairing check alone |

`execute` and the re-execution are the same work twice: the driver executes once
for the cycle count, then `prove_with_mode` executes again because it proves from
the ELF rather than from a proof. Dropping `--mode core`'s instrumentation would
save ~26 s.

## What the precompiles and retention bought

Comparing run A with run C, same card, same inputs:

| | run A | run C | |
| --- | ---: | ---: | --- |
| cycles | 142,638,298 | 130,367,324 | **-8.6%**, from the Poseidon+Fp precompiles |
| shards | 138 | 84 | **-39%**, from Pasta retention |
| re-execute + STARK | ~4m07s | ~2m36s | **-37%** |
| distinct shard shapes | 55 | 21 | the number that decides whether another proof fits the map |
| VK map programs | 67 | 33 | |

The guest is built from `benchmarks/dev/pasta-poseidon.json`
(`MIZU_POSEIDON_PRECOMPILE=1`, `MIZU_PASTA_FP_PRECOMPILE=1`, plus 40 ZEKO flags).
Retention is `Uint256MulMod` + `VestaAdd` + `PallasAdd` on top of SP1's five
defaults, applied through `SP1WorkerBuilder::with_core_opts`.

Both are compatible with the ceremony-free property: neither the guest ELF nor
the cluster mix is an input to the wrap verifying key.

## What the network volume bought, and what it costs

Volume `mue9i1oojc`, 30 GB, EU-SE-1, STANDARD, at 19 GB used. Measured
sequentially: **636 MB/s write, 835 MB/s read**, against the container disk's
3.3 GB/s and 9.3 GB/s.

| | cold | warm | saved |
| --- | ---: | ---: | --- |
| source upload + extract | ~4.3 min | 0 | on the volume |
| Go toolchain | ~15 s | 0 | symlinked |
| cargo build | 10m23s | **10.5 s** | the CUDA kernels dominate |
| Groth16 artifacts | 2-4 min | 0 | *see the caveat below* |
| proving-key read | 3.8 s | 16.0 s | **costs +12 s** |

Net: a fresh pod goes from ~10 min to ~5m51s, about **$0.10 to $0.06** a proof.
The +12 s on the key read is real and worth stating — the volume is not free,
it is just much cheaper than the download it replaces. Two-to-four minutes is the
observed range of that download across runs A and B; S3 throughput varied.

The cold build is slower on the volume than off it (10m23s against 7m31s on
container disk) because `CARGO_TARGET_DIR` writes to MooseFS. That is paid once.

## Gotchas, each paid for once

**1. The artifacts cannot be installed by SP1 onto network storage.** Its
installer runs `tar -Pxzf` with no `--no-same-owner`
(`prover/src/build.rs:215`), and the tarball is packed on macOS — owner
`fakedev9999:wheel`, uid 501 gid 0, with `._` AppleDouble entries and
`com.apple.provenance` xattrs. The overlay disk permits that `chown`; MooseFS
refuses it, the extract fails, and SP1 reports **"artifact not found"**.

`circuit_artifacts_installed` (`build.rs:639`) checks only for the `.complete`
marker, so extracting it ourselves with `--no-same-owner` and touching that file
makes SP1 skip the download. `install-artifacts.sh` does this: 207 s to download
6,211,807,514 bytes, 94 s to extract 7.9 GB, once per volume.

There is no alternative tarball. The bucket denies listing, and v6.1.0 is what
`SP1_CIRCUIT_VERSION` pins — another version would be another wrap circuit.

**2. The VK map must be built where the proof runs.** `vk-map-local` predicted 24
distinct shapes from 89 records; the pod produced 21 from 84 shards. It had 21 of
the pod's 27 indices and **missed 6**, all the same preprocessed multiple in
clusters it had identified correctly — so it gets the chip mix right and the exact
shard cut wrong. The proof failed with **`vk not allowed`** at the membership
check, loudly rather than silently.

On the pod `--mode vk-map` costs **11 s**; locally it took 10m38s. So
`vk-map-local`'s remaining value is choosing the guest and retention
configuration, and predicting map size, without renting anything.

**3. MooseFS is fast sequentially and slow on many small files.** Extracting the
1.4 GB source (thousands of files) ran at ~7 MB/s; large files run at 636 MB/s.
Do not generalise from the first number — it is what made me wrongly expect the
artifacts to be unusable from the volume.

**4. `vk_root` changes whenever the map changes.** Run A's was `0x0022bcaf..`,
runs B and C `0x0052a857..`. It is a Groth16 public input and the verifier
contract pins it as `VK_ROOT()`, so the map has to be settled before deployment.

**5. Two Groth16 proofs of the same input differ.** Run B and run C produced
different bytes and both verify: gnark's Groth16 proving is randomised.

## Still open

- **Nothing is deployed.** Our `vk_root` differs from Succinct's, so their
  deployed verifier would reject these proofs. `SP1VerifierGroth16.sol` has to be
  deployed with our `VK_ROOT()`; `Groth16Verifier.sol`, the pairing constants, is
  unchanged. Gas is still the ~250,000 estimate, not measured on chain.
- **One input, one map.** Only this Pickles proof has been proved. A
  differently-sharding input needs the map extended, which changes `vk_root`.
  An ELF-scoped map over the 12 clusters this guest reaches is ~2,280 programs,
  about 11 min and $0.11 on an A40.
- **Card class is load-bearing.** The map is valid only for a prover sharding
  identically — VRAM above ~26 GB. A 24 GB A5000/3090/4090 takes
  `local_gpu_opts`' reduced `element_threshold` and would fail the membership
  check.
- **`groth16-cuda` untried.** It targets the 63.5 s gnark prover, ~20% of run C.
  `icicle-gnark` v3.2.2 is source-only and SP1's build links four curves'
  libraries, so it needs a from-source CUDA build.
- **Volume headroom.** 19 GB of 30, with `cargo-registry` at 6.0 GB. A second
  guest ELF plus another artifact set would not fit.
