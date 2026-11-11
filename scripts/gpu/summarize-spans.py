#!/usr/bin/env python3
"""Add up tracing span-close timings from a proof log, per stage.

Why: the 2026-11-11 receipts leave a 40 s hole in every proof.
`consumer_proving` is 108 s while the integrated nvidia-smi series puts
device-busy at 68.6 s, so roughly 40 s is the prover working on the host
between kernels — and nothing in this repository had ever timed its stages.

The GPU crates already instrument themselves (`#[instrument(level = "debug")]`
on `generate_first_layer`, `generate_gkr_circuit` and friends), so running with
`PASTA_SPAN_TIMING=1` and a `debug` filter makes each span print a close event
carrying its own `time.busy` and `time.idle`. This sums them by span name.

Read it as attribution, not as a budget: spans nest, so a parent's busy time
includes its children's, and `time.idle` is time the span existed while its
task was not polled. Compare siblings, and compare a stage against the
device-busy figure from the telemetry series; do not add parents to children.

Usage: summarize-spans.py RUN.log [--json OUT.json] [--top N]
"""
import argparse
import collections
import json
import re
import sys

# tracing's fmt layer prints a close event as
#   <timestamp> LEVEL parent:child{fields}: target: close time.busy=1.23ms time.idle=4.56ms
# The timestamp carries colons of its own, so the line is split on the message
# rather than scanned for the first identifier — scanning found "T07" in the
# timestamp on the first attempt.
PREFIX = re.compile(r"^\S+\s+(?:TRACE|DEBUG|INFO|WARN|ERROR)\s+(?P<rest>.*)$")
TIMES = re.compile(
    r"^(?P<busy>[0-9.]+)(?P<bu>[a-zµ]+)\s+time\.idle=(?P<idle>[0-9.]+)(?P<iu>[a-zµ]+)"
)
MARKER = " close time.busy="
UNITS = {"ns": 1e-9, "µs": 1e-6, "us": 1e-6, "ms": 1e-3, "s": 1.0}


def span_name(left: str) -> str:
    """The innermost span on a close line: the field before the target."""
    m = PREFIX.match(left)
    rest = m.group("rest") if m else left
    parts = [p.strip() for p in rest.split(":") if p.strip()]
    if not parts:
        return "?"
    # `span: target` is the common shape; `a:b:span: target` when spans nest.
    name = parts[-2] if len(parts) >= 2 else parts[0]
    return name.split("{", 1)[0].strip()


def seconds(value: str, unit: str) -> float:
    if unit not in UNITS:
        raise ValueError(f"unknown time unit {unit!r}")
    return float(value) * UNITS[unit]


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("log")
    p.add_argument("--json")
    p.add_argument("--top", type=int, default=25)
    args = p.parse_args()

    busy = collections.Counter()
    idle = collections.Counter()
    hits = collections.Counter()
    matched = 0
    with open(args.log, errors="replace") as fh:
        for line in fh:
            if MARKER not in line:
                continue
            left, _, right = line.partition(MARKER)
            t = TIMES.match(right.strip())
            if not t:
                continue
            matched += 1
            name = span_name(left)
            busy[name] += seconds(t.group("busy"), t.group("bu"))
            idle[name] += seconds(t.group("idle"), t.group("iu"))
            hits[name] += 1

    if not matched:
        print(
            "no span-close events found. Run the proof with PASTA_SPAN_TIMING=1 "
            "and a filter that reaches the stages, e.g. "
            "RUST_LOG=sp1_gpu_logup_gkr=debug,sp1_gpu_shard_prover=debug,"
            "sp1_gpu_basefold=debug,sp1_core_machine=info",
            file=sys.stderr,
        )
        return 1

    rows = [
        {
            "span": name,
            "calls": hits[name],
            "busy_s": round(busy[name], 3),
            "idle_s": round(idle[name], 3),
            "busy_s_per_call": round(busy[name] / hits[name], 4),
        }
        for name in busy
    ]
    rows.sort(key=lambda r: r["busy_s"], reverse=True)

    print(f"{'span':38s} {'calls':>6s} {'busy_s':>9s} {'idle_s':>9s} {'busy/call':>10s}")
    for r in rows[: args.top]:
        print(
            f"{r['span'][:38]:38s} {r['calls']:6d} {r['busy_s']:9.2f} "
            f"{r['idle_s']:9.2f} {r['busy_s_per_call']:10.4f}"
        )
    print(f"\n{matched} close events over {len(rows)} distinct spans.")
    print("Spans nest: a parent's busy time includes its children's. Compare siblings.")

    if args.json:
        with open(args.json, "w") as fh:
            json.dump(
                {
                    "source_log": args.log,
                    "close_events": matched,
                    "note": (
                        "busy_s sums tracing's time.busy per span name; spans nest, so "
                        "parents include children and the column must not be totalled. "
                        "Compare against device-busy from the nvidia-smi series."
                    ),
                    "spans": rows,
                },
                fh,
                indent=2,
            )
            fh.write("\n")
        print(f"wrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
