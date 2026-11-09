#!/usr/bin/env python3
"""Compare two independently recorded runs without confusing execution and proving.

Usage: python3 scripts/devloop/compare.py control.json candidate.json

Schema version 1 is illustrated in run-record.example.json. Replace its example
values and remove ``template`` when recording an actual run. The compatibility
object pins protocol, fixture/VK/SRS/input bytes, toolchain, runtime revision,
backend and workload. Source revisions and program hashes may differ: those
identify the change under test. Correctness reports and raw measurement artifacts
are content-addressed, and the verified public-output hashes must agree.

Execution records compare integer guest cycles only. Core, recursive and wrapped
proof records compare wall seconds only, for identical proof_system and
environment_sha256. A concrete environment description should include hardware,
prover options, concurrency, warm/cold policy and timing boundaries. The workload
must identify the exact invocation, sample/aggregation policy and non-source
configuration. Store those descriptions with the referenced artifacts.

This tool validates record consistency; it does not re-run verification, retrieve
artifacts, authenticate claimed hashes, or establish statistical significance.
Keep independent repetition results and measurement/correctness logs. Imported
historical evidence is never relabeled as fresh Mizu verification.
"""

import argparse
import json
import math
import re
import sys


class RecordError(ValueError):
    """The records cannot support a valid comparison."""


COMPATIBILITY_FIELDS = (
    "protocol", "fixture_sha256", "vk_sha256", "srs_sha256", "input_sha256",
    "toolchain", "runtime_revision", "backend", "workload",
)
KINDS = ("execution", "core-proof", "recursive-proof", "wrapped-proof")


def _object(value, label):
    if not isinstance(value, dict):
        raise RecordError(f"{label} must be an object")
    return value


def _text(obj, key, label):
    value = obj.get(key)
    if not isinstance(value, str) or not value.strip():
        raise RecordError(f"{label}.{key} must be a nonempty string")
    return value


def _hash(obj, key, label, length=64):
    value = _text(obj, key, label)
    if not re.fullmatch(rf"[0-9a-f]{{{length}}}", value) or set(value) == {"0"}:
        raise RecordError(f"{label}.{key} must be a nonzero {length}-digit lowercase hex hash")
    return value


def validate(record, label):
    """Validate one record and return its metric name and value."""
    _object(record, label)
    if type(record.get("schema_version")) is not int or record["schema_version"] != 1:
        raise RecordError(f"{label}.schema_version must be 1")
    if "template" in record:
        raise RecordError(f"{label} is a template; record an actual run and remove template")
    _text(record, "run_id", label)
    if record.get("evidence_origin") not in ("observed_mizu", "imported"):
        raise RecordError(f"{label}.evidence_origin must be observed_mizu or imported")
    source = _object(record.get("source"), f"{label}.source")
    _text(source, "repository", f"{label}.source")
    _hash(source, "revision", f"{label}.source", 40)
    _hash(record, "program_sha256", label)
    _hash(record, "measurement_artifact_sha256", label)

    compatibility = _object(record.get("compatibility"), f"{label}.compatibility")
    for key in COMPATIBILITY_FIELDS:
        if key.endswith("_sha256"):
            _hash(compatibility, key, f"{label}.compatibility")
        elif key == "runtime_revision":
            _hash(compatibility, key, f"{label}.compatibility", 40)
        else:
            _text(compatibility, key, f"{label}.compatibility")

    correctness = _object(record.get("correctness"), f"{label}.correctness")
    if correctness.get("passed") is not True:
        raise RecordError(f"{label}.correctness.passed must be true")
    _hash(correctness, "report_sha256", f"{label}.correctness")
    _hash(correctness, "output_sha256", f"{label}.correctness")

    measurement = _object(record.get("measurement"), f"{label}.measurement")
    kind = measurement.get("kind")
    if kind not in KINDS:
        raise RecordError(f"{label}.measurement.kind must be one of {', '.join(KINDS)}")
    if kind == "execution":
        value = measurement.get("cycles")
        if type(value) is not int or not 0 < value <= 2**64 - 1:
            raise RecordError(f"{label}.measurement.cycles must be a positive uint64 integer")
        return "cycles", value
    _text(measurement, "proof_system", f"{label}.measurement")
    _hash(measurement, "environment_sha256", f"{label}.measurement")
    value = measurement.get("wall_seconds")
    if type(value) not in (int, float):
        raise RecordError(f"{label}.measurement.wall_seconds must be a positive finite number")
    try:
        valid = math.isfinite(value) and value > 0
    except OverflowError:
        valid = False
    if not valid:
        raise RecordError(f"{label}.measurement.wall_seconds must be a positive finite number")
    return "wall_seconds", value


def compare(control, candidate):
    control_metric, old = validate(control, "control")
    candidate_metric, new = validate(candidate, "candidate")
    if control["run_id"] == candidate["run_id"]:
        raise RecordError("control and candidate must have distinct run_id values")
    if control["measurement_artifact_sha256"] == candidate["measurement_artifact_sha256"]:
        raise RecordError("control and candidate must reference independent measurement artifacts")
    if control["evidence_origin"] != candidate["evidence_origin"]:
        raise RecordError("cannot mix imported historical evidence and observed Mizu evidence")
    for key in COMPATIBILITY_FIELDS:
        if control["compatibility"][key] != candidate["compatibility"][key]:
            raise RecordError(f"incompatible {key}")
    if control["correctness"]["output_sha256"] != candidate["correctness"]["output_sha256"]:
        raise RecordError("verified public output hashes differ")
    kind = control["measurement"]["kind"]
    if kind != candidate["measurement"]["kind"] or control_metric != candidate_metric:
        raise RecordError("measurement kinds differ; execution and proof stages cannot be compared")
    if kind != "execution":
        for key in ("proof_system", "environment_sha256"):
            if control["measurement"][key] != candidate["measurement"][key]:
                raise RecordError(f"incompatible {key}")
    delta = new - old
    delta_percent = (new / old - 1) * 100
    if not math.isfinite(delta) or not math.isfinite(delta_percent):
        raise RecordError("comparison overflows; metrics cannot be represented safely")
    return {
        "schema_version": 1,
        "comparison_valid": True,
        "evidence_origin": control["evidence_origin"],
        "control_run_id": control["run_id"],
        "candidate_run_id": candidate["run_id"],
        "measurement_kind": kind,
        "metric": control_metric,
        "control": old,
        "candidate": new,
        "delta": delta,
        "delta_percent": delta_percent,
        "reduction_percent": -delta_percent,
        "improved": new < old,
        "interpretation": (
            "Guest execution cycles only; no proof-time or proving-support conclusion."
            if kind == "execution" else
            "Recorded proof wall time for this proof stage/environment; repeated measurements required."
        ),
        "verification_scope": "Record consistency only; artifacts and correctness claims are not independently authenticated.",
    }


def _pairs(pairs):
    obj = {}
    for key, value in pairs:
        if key in obj:
            raise RecordError(f"duplicate JSON key: {key}")
        obj[key] = value
    return obj


def read_record(path):
    def reject_constant(value):
        raise RecordError(f"non-finite JSON constant: {value}")
    with open(path, encoding="utf-8") as stream:
        return json.load(stream, object_pairs_hook=_pairs, parse_constant=reject_constant)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("control")
    parser.add_argument("candidate")
    args = parser.parse_args()
    try:
        result = compare(read_record(args.control), read_record(args.candidate))
    except (RecordError, OSError, ValueError) as error:
        print(json.dumps({"comparison_valid": False, "error": str(error)}))
        return 2
    print(json.dumps(result, indent=2, allow_nan=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
