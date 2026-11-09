//! CPU-only support for the proving harness: execution-record accounting and
//! the manifest that pins a core-proof run's identity and options. No proving
//! entry points — those live in `main.rs` and, behind the `cuda` feature, in
//! `gpu.rs`, neither of which this library can reach.
pub mod run_manifest;
pub mod trace_accounting;
