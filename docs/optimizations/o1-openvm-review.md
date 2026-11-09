# External review: o1-openvm

Reviewed 2026-11-09. This is external research, not an imported Mizu optimization
or a new benchmark. The [catalog](../../benchmarks/references/o1-openvm-review.json)
records the seven main commits, dependency revisions, source blob identities,
reported figures and reproduction limits.

The reviewed guest is [`3724a3d`](https://github.com/youtpout/o1-openvm/commit/3724a3d12178c3a174513ad06e8896d700f56d64),
declaring OpenVM **2.0.1**. Important implementation changes live in its linked
`youtpout/proof-systems` and `youtpout/o1js-to-zkvm` forks.

## Optimization inventory

| Area | Work | Evidence / qualification |
| --- | --- | --- |
| Guest scheduling | Disable parallel verifier features; isolate host input generation from guest feature resolution | [Port/configuration](https://github.com/youtpout/o1-openvm/commit/9aa7b5c1c435290777041b8d5a128a3e3bcc4b7a). No isolated OpenVM speedup established here. |
| Setup | Embed protocol SRSes and precomputed Lagrange bases, avoiding guest curve FFT setup | [Builder](https://github.com/youtpout/o1-openvm/blob/3724a3d12178c3a174513ad06e8896d700f56d64/build.rs). VK remains a runtime input. |
| Field arithmetic | Route Pasta Fp/Fq Montgomery operations to OpenVM modular arithmetic; amortize the Montgomery correction across a sum of products | [Field adapter](https://github.com/youtpout/proof-systems/commit/788cb68f7cfd85bf443bc144fd1879ce88a581d6). |
| Vesta MSM | Bridge accumulator point/scalar representations to the ECC extension | [Verifier adapter](https://github.com/youtpout/o1js-to-zkvm/commit/1269196aceffc89e1a27bbab6885131df10ae93d). |
| Pallas MSM | Route the IPA opening MSM through the ECC extension, with ordinary fallback for other curves | [Commitment adapter](https://github.com/youtpout/proof-systems/commit/e0a135240e342483d461e7319b4b7cb32425148f). |
| Allocation | Replace temporary byte vectors in coordinate/scalar conversion with direct limb access | [Root change](https://github.com/youtpout/o1js-to-zkvm/commit/276f324afc48ec74d3db4c7f276cfbd35530b005), [dependency change](https://github.com/youtpout/proof-systems/commit/5ba1f140eefd7c3bf8c8ac38c878d248c9925183). Reported final gain includes added VK validation; not an allocation-only ablation. |
| Multi-proof accumulation | Combine accumulator equations with Fiat–Shamir weights, replacing N large MSMs with one large plus a small MSM | [Batching commit](https://github.com/youtpout/o1js-to-zkvm/commit/146db80719b4cb76c2b50c030d72bee3b029664c). Reported host-phase speedups 1.81× at N=2, 2.83× at N=3, with ~17% timing variation. The universal entry point is single-proof, so this does not explain its headline figure. |
| Public I/O | Publish a Keccak digest of ABI-encoded VK identity and statement, including its length | [Guest entry point](https://github.com/youtpout/o1-openvm/blob/3724a3d12178c3a174513ad06e8896d700f56d64/src/main.rs). Output/consumer design; no isolated proving-time gain measured. |

`openvm_ecc_guest::msm` is a
[software Pippenger loop](https://github.com/openvm-org/openvm/blob/b820b25baab6c5d9b055f64e0286b6b1058e707c/extensions/ecc/guest/src/msm.rs)
over accelerated point operations. There is no whole-MSM chip demonstrated by
that API call.

## Reported measurements

These are the author's instruction/trace-cell measurements for one mainnet
Mina proof, read from the [recorded sequence](https://github.com/youtpout/o1-openvm/blob/3724a3d12178c3a174513ad06e8896d700f56d64/CLAUDE.md).
They were not reproduced during this review.

| Configuration | Instructions | Reported trace cells |
| --- | ---: | ---: |
| No Pasta acceleration | 31,819,681,513 | 1,170,322,141,177 |
| Modular arithmetic | 24,221,887,063 | 896,055,841,422 |
| Plus Vesta ECC | 5,815,088,237 | 220,926,602,404 |
| Plus Pallas ECC, before additional point checks | 2,249,380,517 | 86,644,291,990 |
| Checked point construction | 2,286,997,815 | 88,224,421,019 |
| VK checks and allocation removal | 2,230,979,102 | 85,934,163,225 |

The final count is ~14.26× below that OpenVM baseline. It does not measure a
14.26× proof-time improvement or an advantage over Mizu.

## Correctness and reproduction notes

The port adds checked point conversion, runtime VK commitment checks, release
length assertions, identity handling and two malformed-input guest controls.
The later [validation correction](https://github.com/youtpout/o1-openvm/commit/1be92393f09807d202031b569e73698a56f8048d)
withdraws an earlier claim about missing guest input validation: the serialized
proof points already used validating deserialization. Do not repeat that claim.

The latest committed guest lockfile contains source-less/path-style verifier
entries while the manifest declares Git dependencies, and still records
proof-systems `e0a1352`, preceding the observed allocation/hardening tip
`5ba1f14`. Some guest comments also describe an obsolete unaccelerated state.
Therefore the final reported figure needs a clean locked rebuild and new
source/ELF/input/output evidence before treating it as a reproduced baseline.
No build failure was independently reproduced here. The early proof-generation
commit predates the Mina port and does not prove the optimized Mina guest.

## OpenVM versus our SP1 setup

| Aspect | OpenVM in this port | Mizu / SP1 6.10 |
| --- | --- | --- |
| Guest ISA | RV32IM | RV64IM |
| Proof system generation | OpenVM 2.x SWIRL | SP1 V6 Hypercube |
| Pasta integration | Configure existing modular/ECC extensions; bridge Arkworks types in verifier/dependency forks | Maintain custom Pasta syscall/AIR/event integration in the SP1 fork |
| MSM scheduling | OpenVM guest Pippenger around ECC operations | Tuned GLV, positional tables, borrowed buffers and point operations |
| VK/output policy | Runtime VK; 32-byte statement/VK digest in this configuration | Embedded verifier setup; boolean core output |

OpenVM's [extension architecture](https://docs.openvm.dev/book/advanced-usage/creating-a-new-extension/)
and [configurable ECC](https://docs.openvm.dev/book/acceleration-using-extensions/elliptic-curve-cryptography/)
are useful for maintaining additional curves without modifying its core. SP1
also has accelerated chips and per-instruction AIRs; it is inaccurate to describe
SP1 V6 as one central CPU chip. Its
[Hypercube protocol](https://docs.succinct.xyz/docs/sp1/hypercube/) and
[64-bit migration](https://docs.succinct.xyz/docs/sp1/getting-started/migration)
are distinct from OpenVM's
[SWIRL generation](https://blog.openvm.dev/2.0-production).

The 32-byte output describes this port/default aggregation configuration, not a
universal OpenVM limit: the [SDK supports custom system configuration](https://docs.openvm.dev/book/writing-apps/generating-proofs/).

Their SP1 reference is 4,378,867,074 cycles. Our
[current result](../../benchmarks/core-proof-systems-0.7-sp1-6.10.json) is
142,650,295 with a different guest/setup/output policy. Neither their ~1.95×
claim nor a quotient of our counts is a matched VM comparison. Instructions,
trace cells and end-to-end proving latency require separate measurements.

For Mizu, the batching algorithm is a concrete candidate to evaluate for
multi-proof batches; current accumulator checks run per proof. Its transcript,
adversarial cases and interaction with our fixed tables need review before any
port. The single-proof benchmark would not benefit. Most other principles are
already represented in our [area records](README.md). No code was imported.
