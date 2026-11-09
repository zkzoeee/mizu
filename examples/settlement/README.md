# Optional settlement receipt example

This standalone workspace layers versioned receipt derivation over a verified
Pickles proof. The root verifier, guest and host do not depend on it. It contains
receipt data types and the receipt guest, with the inherited bridge-transition
API removed. Protocol domain strings, wire schemas and V1–V4 receipt encodings
are preserved for compatibility. This is an example, not a deployment stack.

```sh
CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 cargo test --profile native-test --locked --manifest-path examples/settlement/Cargo.toml --workspace --lib -- --test-threads=1
```

Build the receipt guest from `examples/settlement/program` with SP1 6.10. It reads
a `VerifiableProof` followed by `Option<SettlementWitnessV1>`. `None` emits the
historical 577-byte compatibility output; a witness emits its versioned receipt.
The core guest instead reads only the proof and emits boolean true.

`crates/cycle-runner` accepts `--guest-abi settlement` to execute the historical
`None` input/output shape explicitly. Receipt derivation and binding tests cover
witnessed receipts. The historical 6.1/6.10 cycle comparison is retained under
`benchmarks/reference`; a new guest build must receive a fresh measurement.
