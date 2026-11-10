# GPU proof drivers

Five scripts that take the pinned core guest to a rented GPU and bring the
evidence back. They are a port of wazabi's r123 drivers, which proved this same
harness binary on an RTX 5090; the runbook, the measured costs and the reasons
behind each choice are in [the runbook](../../docs/gpu-run-on-runpod.md).

| Script | Runs on | Does |
| --- | --- | --- |
| `make-bundle.sh` | the Mac | checks the ELF and all three inputs against the manifest, then packs the harness, the `vendor/sp1` fork and the artifacts |
| `push-and-launch.sh` | the Mac | direct-SSH probe, scp, hash check on the pod, arms the watchdog and launches under tmux |
| `pod-run.sh` | the pod | gates the GPU, unpacks, reuses or builds the CUDA harness, pre-flights the manifest, proves each arm with telemetry, verifies every proof from disk |
| `cache-key.py` | the pod | the build's identity (OS, nvcc, rustc, `CUDA_ARCHS`, sources), so a volume's cached binary is only reused when rebuilding would be pointless |
| `watchdog.sh` | the pod | halts the container at a wall-clock deadline, because the cost ceiling must not depend on an agent staying attached |
| `collect-results.sh` | the Mac | pulls receipts first, then the proof bytes, hash-checked on both sides |

Nothing here is wired into CI or into `scripts/dev.py`, and nothing runs
without a host, a port and a deadline given explicitly. `pod-run.sh` sets
`PASTA_ALLOW_REAL_PROOF=1` only after every pinned identity has been checked.
