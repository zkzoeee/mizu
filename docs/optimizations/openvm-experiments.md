# OpenVM-inspired Mizu experiments

These are independent adaptations of the ideas in [the external review](o1-openvm-review.md),
not imports of its dependency forks. SP1 remains 6.10.0; all three dependency pins
remain unchanged. The retained configuration and existing measurements are controls.

## Scope and ownership

| ID | Mechanism | Implementation / selection |
| --- | --- | --- |
| mizu-openvm-allocation | Stack cross-field bytes; iterate CIP evaluation columns without transpose vectors | Root deferred.rs / experimental_cip.rs; MIZU_STACK_CROSS_FIELD=1 and MIZU_CIP_MODE=borrowed |
| mizu-openvm-fused-cip | Two evaluation-point dot products in four-term sum_of_products blocks | Same files; MIZU_CIP_MODE=fused, with stack conversion enabled |
| mizu-openvm-batch-accumulator | Transcript-weighted accumulator coefficient combination; one large MSM and a small commitment MSM | Root accumulator_batch.rs / serialize.rs; bounded batch guest; MIZU_BATCH_ACCUMULATOR=1 |
| mizu-openvm-statement-digest | Keccak digest binding the exact embedded VK JSON identity, statement length and canonical fields | Root attestation.rs; o1-verifier statement-digest feature |
| mizu-openvm-trace-accounting | Bounded replay estimates, event rows and measured runner RSS | cycle-runner --profile-precompiles; no proving APIs |

All variants are opt-in. The normal single-proof guest keeps its boolean ABI.
The ordinary CIP and per-proof accumulator remain reference implementations.
The root owns Pickles-specific schedules and output policy; existing algebra
sum_of_products and SP1 instrumentation are reused without editing their forks.
When syncing upstream, replay these separate root commits and re-run equivalence
and guest validation against the newly pinned dependency implementations.

## Validation and measurement recipe

`scripts/openvm_experiments.py all --output /absolute/new/directory` builds frozen
control/candidate guests and a runner, then executes honest and corrupted inputs.
Phases build/single/batch/profile can be run separately. Completed executions are
reused only with matching source, script, runner, ELF, fixture and result hashes.

The guest batch count is checked before allocating proof storage, bounded to 1..8.
The batch algorithm rejects empty inputs, wrong SRS length and coefficient geometry;
it binds the ordered challenges/commitments, VK digest, count and SRS domain into
a domain-separated Fiat–Shamir challenge, explicitly excluding zero. Native checks
include corruption in every position, opposite cancelling errors, order binding,
truncated SRS, and round-count overflow. This is an experimental construction,
not an independent cryptographic audit.

The cycle batch workload repeats the one available mainnet proof N times. N=1,
2,4,8 compare the same input sequence/output on the same batch guest ABI, using
per-proof versus combined accumulators. These are not diverse production batches.
The batch guest loads the complete embedded wrap setup; its N=1 total must not be
used as an isolated batching comparison with the prefix-trimmed ordinary guest.

The digest wire is Keccak256(ASCII `mizu/verified-statement/v1` || VK-id[32] ||
field-count-u64-LE || canonical-field-u256-LE...). VK-id is SHA256 of the exact
embedded VK JSON. It is versioned Mizu encoding, not Ethereum ABI or OpenVM's
attestation format. The guest emits exactly 32 digest bytes only after successful
verification. The runner computes and checks the expected output, and statement,
accumulator and opening corruptions must halt unsuccessfully before any output.

Compilation, native tests, CPU execution, replay/event estimates and runner RSS
are separate evidence. None establishes fresh proof generation, complete proof
verification, from-disk proof verification, CUDA cost or end-to-end proving time.
Per-child wait4 resource accounting measures runner/JIT RSS; it is not prover memory.
The initial time -l wrapper failed after successful control execution because an
OS clockrate query was disallowed. Its raw result/log and original identity receipt
are preserved outside the completed matrix; the replacement changes no guest or
runner bytes.

## Results

Measurements are in [the machine-readable matrix](../../benchmarks/openvm-experiments-sp1-6.10.json).
All **36 frozen CPU runs** passed: **15 accepted** executions and **21 rejected**
malformed-input executions. Each rejected guest halted unsuccessfully with no
public output. All three trace replays matched their unprofiled cycles exactly.

| Single-proof configuration | Cycles | Difference from fresh control |
| --- | ---: | ---: |
| Control | 142,638,298 | — |
| Stack conversion / borrowed CIP | 142,553,240 | −85,058 (−0.0596%) |
| Plus fused four-term CIP blocks | 142,556,429 | −81,869 (−0.0574%) |
| Statement/VK digest output | 142,645,115 | +6,817 (+0.0048%), changed output ABI |

The fused version uses 66 fewer UInt256 calls than the allocation variant,
but costs 3,189 more guest cycles. It remains a prototype; fewer calls did not
establish a faster guest. The allocation variant's saving is entirely in wrap
preparation. All four compiled guests accepted the honest mainnet proof and
rejected both accumulator/opening corruptions with unsuccessful halt and empty
output. The digest guest also rejected a changed application statement.

| Proofs | Per-proof accumulator control | Combined accumulator | Whole-guest cycle reduction |
| --- | ---: | ---: | ---: |
| 1 | 142,664,462 | 142,580,188 | Fallback; no mathematical batching saving |
| 2 | 241,406,025 | 173,244,385 | 28.2353% |
| 4 | 435,999,550 | 229,664,187 | 47.3247% |
| 8 | 825,188,525 | 342,539,805 | 58.4895% (2.4090×) |

At N=4, Vesta additions fall from 5,242,732 to 1,310,696, while UInt256 calls
increase from 2,278,159 to 2,486,641. The accumulator stage falls from
301,239,024 to 94,856,655 cycles. This is CPU execution and call-count evidence,
not a measured proving-time improvement. The N=1 call counts are unchanged;
its instruction difference reflects build/control-flow effects, not a removed MSM;
no isolated arithmetic saving is claimed for this fallback.

The fresh control differs by −11,997 cycles from the earlier 142,650,295 core
checkpoint. That source/code-generation change is not credited to a new
optimization. All candidate deltas above use fresh matching controls.

Prototype source commits: 7f9d257 (allocation/CIP), 4fde221 (batch), 50eaf97
(digest), acbdce5 (measurement tools), 92d711c (child resource accounting).
Guest sources were frozen at 00d0347 with complete per-file hashes in each build
receipt. The resource-accounting repair changes no guest or runner bytes.
No retained defaults or dependency pins have changed.

### Reporting corrections

The original frozen build configs inherited the boolean ABI/expected-output
metadata for the optional guests. Their selected features, binary and actual
runner ABI/output identify the executed contracts; the summary records an
`effective_guest_abi`. The original runner's legacy `proof_valid` field also
interprets the first digest byte as a boolean. The authoritative `accepted`
verdict already checks the exact expected 32-byte digest. Raw receipts remain
unchanged; these metadata fields were corrected separately in 4dbab30 after the frozen
matrix, without changing the verifier/guest algorithms. The generic benchmark
now also passes the configured ABI, pins the fixture VK, clears inherited
experiment flags, and captures untracked source plus example/tool/recipe inputs.
The corrected digest run deliberately injected conflicting parent experiment
flags and reproduced an identical guest ELF, the original exact digest and
142,645,115 cycles, with correct ABI/validity reporting.
Its build/result hashes and completion status are separate from the frozen matrix.

## Trace and memory observations

| Configuration | UInt256 rows | Total precompile rows | SP1 gas estimate |
| --- | ---: | ---: | ---: |
| Control | 533,878 | 2,543,030 | 533,365,470 |
| Allocation | 533,784 | 2,542,936 | 533,304,347 |
| Fused | 533,718 | 2,542,870 | 533,305,352 |

All profiles replayed 117 chunks with one live trace buffer of 18,649,847 bytes.
These are unpadded precompile rows and SP1's normalized gas model, not complete
trace-cell counts, proof time or Ethereum gas. Fused saves 66 additional UInt256
rows versus allocation, but its cycles and gas estimate are slightly worse.
Allocation is the better measured single-proof candidate on both metrics.

The eight-proof runner peaked at 338.17 MiB for the control and 339.33 MiB for
the combined version. Its one-shot CPU execution wall times were 682.67 and
136.32 seconds respectively. These describe the local runner and host arithmetic;
there are no repetitions/spread or prover wall-time/RSS measurements. The cycle
ratio is 2.4090×, distinct from the emulator's wall-time ratio.

The combined batch's corrupted-accumulator checks at N=3 all cost about 104.1M
cycles. The per-proof control rejects an invalid first member at 86.2M but an
invalid last member at 241.4M. Batching trades early invalid-member rejection
for shared honest-batch work; the guest remains bounded to eight members.

## Validation and decision

- **214 core/runner native tests:** the 213-test experimental suite plus a new
  distinct-producer integration test. The latter uses real simplechain wrap0/1/2
  proofs under one VK, all 40 retained settings and the three experimental flags.
  It checks complete embedded-batch acceptance, corrupted challenges/openings in
  every member, swapped commitments, a rehashed statement and another VK.
- **20 receipt-example native tests** and an explicit no-std verifier check.
- Six compiled experimental guest variants and the 36-run CPU matrix above.
- The separate corrected digest/ambient-flag recheck passed; its immutable
  build/result hashes are in the JSON post_validation section.
- No fresh SP1 proof generation, proof verification/reload or CUDA run.

Keep **allocation/borrowed CIP** as the small single-proof candidate; retain
**fused CIP** as a measured cycle/gas regression against that candidate.
The **combined accumulator** is the largest throughput candidate for multi-proof
workloads, with the repeated-input timing limitation above. The **digest output**
is a separate functional option with low measured overhead. All remain opt-in;
`benchmarks/retained.json` and the three dependency pins are unchanged.

Before promotion, run selected other producer fixtures through compiled candidate
guests, use diverse real batch inputs for throughput measurements, and explicitly
schedule the applicable fresh SP1 proof/complete-verifier gates. The native
distinct-proof check adds correctness coverage; it does not turn the repeated
mainnet timing workload into a diverse production benchmark.

See also the [Miza validation review](miza-validation-review.md) for stronger
comparison/proof-handoff gates and the separate pending runtime-AIR review.

## Soundness review against Zakura's prepared zero-check rules

Reviewed 2026-11-10 (`mizu-zakura-batch-review`): the combined accumulator's
challenge in `accumulator_batch.rs` checked against the rules Zakura Common
documents for its batched fixed-base zero checks (`pasta_curves/src/glv/zero.rs`):

| Rule | Combined accumulator | Evidence |
| --- | --- | --- |
| Challenge derived only after every equation is fixed | Yes: Blake2s over each member's raw challenges and commitment, in order, length-prefixed | Source; new binding test |
| Challenge binds every input that defines an equation | Count, SRS size, VK digest, each challenge vector and commitment | `experimental_batch_challenge_binds_every_transcript_input` |
| Challenge binds the prepared bases' identity | **No.** The Vesta bases and fixed table are not hashed | Source |
| Zero challenge excluded | Yes, by counter rejection | Same test |
| Prepared tables never deserialized from untrusted input | Yes: build-derived, embedded, bound to the SRS bytes | Existing embedded-setup tests |
| Soundness error stated | Combination of `n` equations: at most `(n - 1)` times the largest challenge-value probability, about `n * 2^-254` | Arithmetic |

The one gap is defense in depth, not an attack. The bases are program-fixed
before any proof exists, and a caller who controlled the bases would control
verification anyway. Zakura binds them so that one prepared check can never be
reused with different bases; binding a build-time digest of the step SRS into
the transcript would do the same here at no runtime hashing cost. That changes
the experimental transcript (`v1` to `v2`), so it is recorded as a
recommendation rather than made unasked. Early rejection of an invalid first
member is lost under batching, as already noted above.
