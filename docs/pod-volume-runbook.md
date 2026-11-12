# Pod runs on the network volume

The first Groth16 run (2026-11-12, no volume) took 24 minutes of pod time, of
which only ~8 were the proof. The rest was a cold build (7m31s), the 7.9 GB
artifact download (~138 s), toolchain installs and the source upload. All of that
is reusable, so it lives on a network volume now.

## The volume

`mue9i1oojc`, **mizu-groth16-cache**, 30 GB, **EU-SE-1**, STANDARD.
Billing measured from its predecessor: ~$0.0175-0.022/day per 10 GB, so this one
costs roughly **$1.60-2.00/month** until deleted.

It replaced `zvx1pq06vk` (10 GB), which held the `pasta-proof` harness build
cache. That cache is gone and will cost ~12.5 min to repopulate if the harness is
used again; 10 GB could not also hold 7.9 GB of Groth16 artifacts.

**EU-SE-1 is not an arbitrary choice.** A volume is data-center-local, so the pod
must land in the same DC, and the DC has to have a card that shards the way the
VK map expects. EU-SE-1 has the A40 and the A6000 (48 GB each). CA-MTL-1 is
HIGH_PERFORMANCE-only storage and its other cards are an H100 and a **24 GB
A5000** — and a 24 GB card takes `local_gpu_opts`' reduced `element_threshold`
(`ceil(vram/GB) + 4 <= 30`), shards differently, and would fail the VK map's
membership check. See `vk-map-local`'s doc comment.

Both EU-SE-1 cards are LOW availability, so expect refused creates; retry the
*create* rather than re-reading availability.

## Layout

```
/workspace/
  sp1/              -> symlinked as /root/.sp1     7.9 GB  the released Groth16 artifacts
  cargo-registry/   -> /root/.cargo/registry       0.8 GB  crate sources
  target/            = CARGO_TARGET_DIR            2.4 GB  warm build, incl. the CUDA kernels
  src/mizu/                                        ~1 GB   the checkout
  src/mizu-gpu-input/                                      proof + receipt inputs
  elf/                                             26 MB each guest ELF
  vkmap/                                                   partial VK maps
  out/                                                     proofs and reports
```

About 13 GB used of 30.

`~/.sp1` has to be a symlink rather than an env var: SP1 resolves it through
`dirs::home_dir()` with no override (`prover/src/build.rs:153-167`). The crate
registry is symlinked on its own rather than moving `CARGO_HOME`, which also
holds the cargo shims rustup installs at boot.

## Running

Attach the volume at pod create:

```json
"mounts": { "network": [ { "volumeId": "mue9i1oojc", "path": "/workspace" } ] }
```

Then on the pod:

```sh
/root/vol-setup.sh                       # idempotent; wires the symlinks, prints what is cached
ELF=/workspace/elf/<guest> MAP=/workspace/vkmap/<map>.bin /root/vol-run.sh
```

`vol-run.sh` leaves `SP1_CIRCUIT_MODE` unset, so the production path installs
Succinct's released artifacts — which, after the first run, are already on the
volume. It passes `--retain-pasta true`, which **must** match the setting the VK
map was built with, because retention changes how shards are cut.

## What each run should cost

| | first (populating) | subsequent |
| --- | --- | --- |
| boot + prereqs | ~2 min | ~2 min |
| source upload | ~50 s | skipped (on the volume) |
| cargo build | ~7.5 min | ~20 s, nothing to recompile |
| artifact download | ~2.3 min | skipped |
| VK map | uploaded, built locally | skipped |
| proof | ~6 min | ~6 min |
| **total** | **~19 min, ~$0.19** | **~8 min, ~$0.08** |

The proof itself is then most of the bill, which is the point.

## Caveats

- The volume is **not** a substitute for the repo: `/workspace/src/mizu` is a
  copy and drifts. Re-upload after changing the drivers or the fork.
- `CUDA_ARCHS=86` in `vol-run.sh` builds kernels for Ampere only. A 5090 (sm_120)
  would need `CUDA_ARCHS=120` and a separate target dir, since the cached build
  would otherwise be reused and fail at runtime.
- Deleting the volume loses the artifacts, the warm build and the registry;
  budget ~10 min to repopulate.
