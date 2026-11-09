//! Dedicated CPU-only accounting binary; cannot dispatch proving modes.
fn main() -> anyhow::Result<()> {
    pasta_precompile_proof::trace_accounting::cli()
}
