# Three levers on the Groth16 wrap, measured

2026-11-13, one 2xA40 pod in EU-SE-1 (16.15 cores, $1.18/hr), same volume
`mue9i1oojc`, same guest ELF `dc8e4b40..`, same 33-key VK map, same input.
Both proofs verified, both carry `vk_root`
`146043302688896039011755426089877291133476984444833909276968832339532477972`
and `groth16_vkey_hash` `4388a21c..` -- Succinct's released `VERIFIER_HASH`.

| | run C | run A (`run-debug`) | run B (`run-notags`) |
| --- | ---: | ---: | ---: |
| GPUs / cores | 1xA40 / 7.65 | 2xA40 / 16.15 | 2xA40 / 16.15 |
| go tags | `debug` | `debug` | none |
| redundant `execute` | yes | yes | **no** |
| wall clock | 5m50.9s | 5m05.8s | **4m39.2s** |
| `execute` | 26.0 s | 23.8 s | **0.0 s** |
| `prove` | 294.5 s | 254.2 s | 250.9 s |
| gnark read R1CS | 24.0 s | 29.4 s | 27.0 s |
| gnark read proving key | 16.0 s | 20.1 s | 20.3 s |
| gnark solver | 25.4 s | 12.9 s | 12.8 s |
| gnark prover | 63.5 s | 31.3 s | 31.5 s |
| peak cgroup memory | -- | 46.4 GiB | 46.3 GiB |

Run C is `../groth16-poseidon-retained-20261113/record.json`, carried here for
comparison. The memory figure is `/sys/fs/cgroup/memory.current`, so it counts
the page cache of 8 GB of artifacts as well as the process; it is not RSS.

## 1. Cores: a large win on time, a loss on cost

Doubling the cores halved both CPU-bound gnark stages -- solver -49%, prover
-51%, close to linear. But 2xA40 is twice the hourly rate for 20% less wall
clock, so **cost per proof rises**. Take the cores when latency matters; stay on
1xA40 when cost per proof does.

The two read stages went the wrong way (R1CS 24.0 -> 27.0 s, key 16.0 -> 20.3 s),
eating about a quarter of the gain. That is MooseFS variance rather than
anything about cores, but it was not isolated.

## 2. The `debug` go tag: no effect at all

Solver 12.9 -> 12.8 s, prover 31.3 -> 31.5 s, "Generating proof"
44.28 -> 44.27 s. Noise. The debug information that costs 24 s to read is
inside Succinct's released R1CS artifact, so no build tag can drop it -- only a
reader that skips it, which is what succinctlabs/sp1#3017 adds (circuit read
11.0 -> 0.77 s). `SP1_GNARK_GO_TAGS` in the fork stays because it is what made
this measurable.

## 3. Dropping the redundant execute: -26.6 s, as predicted

`prove_with_mode` re-executes from the ELF regardless, so the driver's own
up-front execution was the same work twice. `--count-cycles` now defaults to
false and the record reports `cycles: null`. Wall 5m05.8s -> 4m39.2s.

## The process error this run exposed

Run A silently proved with the *previous* binary: the `--count-cycles` edit
landed in the wrong scope, the build failed with exit 101, and `vol-run.sh`
only checked that the binary existed. The run looked normal and was reported as
normal. `vol-run.sh` now deletes the binary before building.
