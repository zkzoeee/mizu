# Cached CPU development loop

Use `scripts/dev.py` for routine optimization work. It keeps SP1 at 6.10 and the
existing dependency forks/pins. No command requests an SP1 proof, GPU or network
prover. The receipt application remains separate.

## Daily iteration

```sh
make dev-tools
make dev-check DEV_RECIPE=benchmarks/dev/allocation.json
make dev-check DEV_TEST=point_spikes
make dev-operation
```

`dev-tools` tests the comparator, cache integrity and independent Poseidon integer
oracle without a guest build. `dev-check` selects the recipe's native integration
tests or library filters, applies that recipe's exact compile-time settings,
uses native-test optimized arithmetic and runs no-std checking. A filter matching
zero tests fails. All builds, Rayon workers and test threads are limited to one.
Use `--all` / `make dev-check-all` for the complete verifier/runner suite.
Combining `--all` with `--test` or `--filter` is rejected.

The small Poseidon guest has no embedded SRS or verifier build script. It commits
all six field outputs and the input iteration count. The CPU operation runner
checks ordinary Mina arithmetic, while the controller additionally checks an
independent Python integer oracle over the published constants. The checked-in
micro recipes compare ordinary and canonical Poseidon schedules; they screen
kernel changes and do not establish whole-verifier savings.

## Preserve a control before editing source

```sh
python3 -B scripts/dev.py baseline --recipe benchmarks/dev/retained.json \
  --output benchmarks/runs/my-control
# Edit the responsible root module or upstream-based dependency fork.
python3 -B scripts/dev.py check --recipe benchmarks/dev/allocation.json
python3 -B scripts/dev.py bench \
  --control-record benchmarks/runs/my-control/baseline.json \
  --recipe benchmarks/dev/allocation.json --output benchmarks/runs/my-candidate
```

A baseline freezes source bytes, ELF, runner, fixture/input/output identities and
the observed cycles. `bench` preserves its runner across a source edit and checks
compatible protocol, toolchain, runtime source, input and verified output before
comparing. A changed runtime requires a new matched baseline. For two features
on the same source, use `--control benchmarks/dev/retained.json` instead of a
frozen record. `make dev-bench` compares the retained and allocation recipes.

A verifier comparison also requires a relevant rejected candidate input. Select
it with `--negative`; the default is accumulator corruption. The controller
requires changed serialized input, an unsuccessful **guest** halt and no public
output. A host failure or bad log is not accepted as a verifier rejection.
Changed-output ABIs cannot be compared as performance-only changes.

## Promotion checks

```sh
make dev-gate DEV_RECIPE=benchmarks/dev/retained.json
```

The gate verifies clean pinned dependency forks, tests the Python tooling, runs
source/flag-matched full native and no-std checks, then executes the honest mainnet
proof, six witness mutations, a separately baked wrong VK, all nine producer
fixtures and bounded trace replay. Mutations cover statement, rehashed statement,
previous commitment, accumulator, public input and outer opening. Wrong-key
checks preserve proof input and require a different ELF. Producer families with
byte-identical VKs share guest builds; every proof retains its own input hash.
Each new execution also freezes all four input files in its own checked artifact
directory. Operation observations retain the integer input and oracle result.

An exact full native receipt can be supplied with `--native-record`; its source,
flags, compiler/config identity and logs must match. `--case` and `--fixture`
select subsets. `--skip-native` and `--skip-profile` are explicit partial checks.
Reused native receipts must include the full-suite and no-std logs, with a
positive test count matching the full-suite log.
Partial selections can pass their requested checks but never claim the complete
CPU matrix. Failed/interrupted entries remain failures, not optimization rows.

For SP1 runtime changes, also use the existing native/AIR/record harness in
`tools/pasta-proof`; its README describes the separate validation layers. The
[Miza review](optimizations/miza-validation-review.md) identifies further
cross-curve/memory-permission and malicious-witness checks worth adapting.
This loop does not import Miza's experimental runtime, change AIRs, or turn
compiled/simulated proof orchestration into a completed cryptographic proof gate.
Fresh SP1 proof generation, complete proof verification/reload and CUDA work
remain separate, explicitly requested checks.

## Cache and evidence rules

The ignored `benchmarks/.dev-cache` stores content-addressed builds, frozen CPU
runners, deterministic source archives, raw logs and executions. Retain it for
later comparison. Never use cache existence as a correctness check: every reuse
checks identities and artifact hashes. Source, settings, builder/compiler,
Cargo configuration and compiler override changes invalidate the relevant key.
Guest/runner artifacts exclude the standalone `tools/` workspaces from their
build scope. Those workspaces are not linked by the root consumer; the full
promotion gate still checks the complete source snapshot for changes.
Compiler overrides are hashed without logging their values. Dirty root/fork
sources are fingerprinted for iteration; the promotion pin check requires fork
changes to be committed, bundled and pinned through the ordinary sync workflow.

The same checkout lock serializes all dev commands across terminals. Guest builds
exclude runner-only changes; proof changes sharing one VK reuse the guest. Cache
hits reuse the original observation, including its run ID and raw-artifact hash;
they are not independent repetitions. `--fresh` deliberately records an additional
execution. Comparing one observation with itself is rejected, even if two recipes
were used to request it. Use new output directories; past reports are immutable.
Comparisons retain both control and candidate bundles and their hashes. Failed
or interrupted commands retain attempt metadata and logs. Inspect and move an
incomplete cache entry aside before retrying; it cannot qualify as a cache hit.

Cold compilation, warm builds, checked cache reuse, guest cycles, CPU wall time,
trace rows and proof time have different scopes. Preserve those distinctions in
[optimization records](optimizations/README.md).

## Validation of this workflow

[Recorded evidence](../benchmarks/dev-loop-validation.json) includes the full CPU
matrix and its checked repeat: 215 native tests, no-std, nine positive fixtures,
six mutations, wrong-key rejection and matching trace replay. The evidence also
separates follow-up storage hardening and representative rechecks. The restored
Poseidon screening comparison took 2.293 seconds when reusing checked evidence;
that timing is one local cache-reuse observation, not a fresh execution benchmark.
After storage hardening, 31 tooling tests and frozen-operation input storage/cache
reuse passed. Concurrent changes to the separate Pasta harness and SP1 runtime
interrupted new build comparisons. Those changes were preserved and require
their own matched baseline and validation; the completed CPU matrix refers to
the earlier archived clean source and executables.
