//! Bounded concurrent shard proving, shared by the CPU and CUDA paths.
//!
//! The previous consumer awaited one `prove_shard` before receiving the next
//! record, so a shard's CPU-side work (main-trace generation, host-side prover
//! work) never overlapped the previous shard's GPU work, and the GPU sat idle
//! whenever the executor was slow. Here up to `depth` shard proofs are in
//! flight at once. Proofs are returned in arrival order; callers still put
//! them into canonical chain order before verification.
//!
//! GPU memory stays bounded by the CUDA prover itself: each `prove_shard`
//! acquires one of the prover's buffers before generating traces, so `depth`
//! above the worker count only queues work on the host.

use std::{sync::Arc, time::Instant};

use futures::stream::{FuturesOrdered, StreamExt};
use serde_json::{json, Value};
use sp1_core_executor::ExecutionRecord;
use sp1_hypercube::{
    prover::{AirProver, PcsProof, ProvingKey, SimpleProver},
    InnerSC, ShardProof,
};
use sp1_primitives::SP1GlobalContext;

use crate::Air;

pub(crate) type CoreShardProof =
    ShardProof<SP1GlobalContext, PcsProof<SP1GlobalContext, InnerSC<Air>>>;

/// Where the consumer's wall clock went, in seconds.
#[derive(Debug, Default)]
pub(crate) struct ConsumerStats {
    pub depth: usize,
    pub records: usize,
    pub max_in_flight: usize,
    /// No proof in flight and waiting for a record: the prover was starved
    /// by the producer. This is the idle term the pipeline exists to remove.
    pub starved: f64,
    /// At least one proof in flight.
    pub proving: f64,
    pub total: f64,
}

impl ConsumerStats {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "prove_depth": self.depth,
            "records": self.records,
            "max_in_flight": self.max_in_flight,
            "starved_seconds": self.starved,
            "proving_seconds": self.proving,
            "total_seconds": self.total,
            "note": "starved: no shard proof in flight while waiting for the producer; proving: at least one in flight",
        })
    }
}

/// Prove every record from `records` with at most `depth` shard proofs in
/// flight, until the channel closes.
pub(crate) async fn prove_stream<C: AirProver<SP1GlobalContext, InnerSC<Air>>>(
    prover: &SimpleProver<SP1GlobalContext, InnerSC<Air>, C>,
    pk: Arc<ProvingKey<SP1GlobalContext, InnerSC<Air>, C>>,
    mut records: tokio::sync::mpsc::Receiver<ExecutionRecord>,
    depth: usize,
) -> (Vec<CoreShardProof>, ConsumerStats) {
    let depth = depth.max(1);
    let started = Instant::now();
    let mut stats = ConsumerStats { depth, ..Default::default() };
    let mut in_flight = FuturesOrdered::new();
    let mut proofs = Vec::new();
    let mut open = true;
    while open || !in_flight.is_empty() {
        let idle = in_flight.is_empty();
        let step = Instant::now();
        tokio::select! {
            // Retire finished proofs first; polling them is also what drives
            // their async stages forward.
            biased;
            Some(proof) = in_flight.next(), if !in_flight.is_empty() => proofs.push(proof),
            record = records.recv(), if open && in_flight.len() < depth => match record {
                Some(record) => {
                    stats.records += 1;
                    in_flight.push_back(prover.prove_shard(pk.clone(), record));
                    stats.max_in_flight = stats.max_in_flight.max(in_flight.len());
                }
                None => open = false,
            },
        }
        let elapsed = step.elapsed().as_secs_f64();
        if idle {
            stats.starved += elapsed;
        } else {
            stats.proving += elapsed;
        }
    }
    stats.total = started.elapsed().as_secs_f64();
    (proofs, stats)
}

/// The producer's phase profile as JSON.
pub(crate) fn producer_profile_json(p: &sp1_core_machine::utils::RecordStreamProfile) -> Value {
    json!({
        "replay_threads": p.replay_threads,
        "chunks": p.chunks,
        "spliced": p.spliced,
        "records": p.records,
        "execute_seconds": p.execute,
        "executor_blocked_seconds": p.executor_blocked,
        "waiting_for_chunk_seconds": p.waiting_for_chunk,
        "splice_seconds": p.splice,
        "trace_chunk_seconds": p.trace_chunk,
        "globals_seconds": p.globals,
        "defer_split_seconds": p.defer_split,
        "dependencies_seconds": p.dependencies,
        "on_record_seconds": p.on_record,
        "total_seconds": p.total,
        "note": "with replay_threads > 1, execute and executor_blocked run on the executor thread and overlap the other columns; on_record is time inside the consumer callback, i.e. blocked on a full channel when streaming to a prover",
    })
}
