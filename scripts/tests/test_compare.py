"""Safety gates for comparison claims; all records here are synthetic test data."""

import copy
import json
import tempfile
import unittest
from pathlib import Path

from devloop.compare import RecordError, compare, read_record


def records(kind="execution"):
    control = {
        "schema_version": 1,
        "run_id": "synthetic-control",
        "evidence_origin": "observed_mizu",
        "source": {"repository": "https://example.invalid/test", "revision": "a" * 40},
        "program_sha256": "a" * 64,
        "measurement_artifact_sha256": "b" * 64,
        "compatibility": {
            "protocol": "synthetic-test-protocol",
            "fixture_sha256": "1" * 64,
            "vk_sha256": "2" * 64,
            "srs_sha256": "3" * 64,
            "input_sha256": "4" * 64,
            "toolchain": "synthetic-toolchain",
            "runtime_revision": "5" * 40,
            "backend": "synthetic-backend",
            "workload": "synthetic fixed-input single run",
        },
        "correctness": {"passed": True, "report_sha256": "6" * 64, "output_sha256": "7" * 64},
        "measurement": {"kind": kind, "cycles": 100, "wall_seconds": 10.0,
                        "proof_system": "synthetic-test-proof", "environment_sha256": "8" * 64},
    }
    candidate = copy.deepcopy(control)
    candidate.update(run_id="synthetic-candidate", program_sha256="c" * 64,
                     measurement_artifact_sha256="d" * 64)
    candidate["source"]["revision"] = "b" * 40
    candidate["measurement"].update(cycles=80, wall_seconds=8.0)
    return control, candidate


class CompareRunsTests(unittest.TestCase):
    def test_execution_allows_new_program_and_revision(self):
        result = compare(*records())
        self.assertEqual(result["delta"], -20)
        self.assertAlmostEqual(result["reduction_percent"], 20)
        self.assertEqual(result["metric"], "cycles")
        self.assertIn("no proof-time", result["interpretation"])

    def test_proof_kinds_have_separate_timings(self):
        for kind in ("core-proof", "recursive-proof", "wrapped-proof"):
            with self.subTest(kind=kind):
                result = compare(*records(kind))
                self.assertEqual(result["metric"], "wall_seconds")
                self.assertEqual(result["delta"], -2)

    def test_every_compatibility_field_must_match(self):
        for key in records()[0]["compatibility"]:
            with self.subTest(key=key):
                control, candidate = records()
                value = candidate["compatibility"][key]
                candidate["compatibility"][key] = ("e" * len(value) if key.endswith("_sha256")
                                                   or key == "runtime_revision" else value + "-different")
                with self.assertRaisesRegex(RecordError, f"incompatible {key}"):
                    compare(control, candidate)

    def test_kind_environment_proof_system_and_output_mismatch(self):
        for section, key, value in (
            ("measurement", "kind", "wrapped-proof"),
            ("measurement", "environment_sha256", "e" * 64),
            ("measurement", "proof_system", "another-proof"),
            ("correctness", "output_sha256", "e" * 64),
        ):
            with self.subTest(key=key):
                control, candidate = records("core-proof")
                candidate[section][key] = value
                with self.assertRaises(RecordError):
                    compare(control, candidate)

    def test_bad_cycle_metrics_rejected(self):
        for value in (True, False, 0, -1, 1.0, float("inf"), float("nan"), "20", 2**64):
            with self.subTest(value=value):
                control, candidate = records()
                candidate["measurement"]["cycles"] = value
                with self.assertRaises(RecordError):
                    compare(control, candidate)

    def test_bad_proof_metrics_rejected(self):
        for value in (True, False, 0, -1, float("inf"), float("nan"), "20", 10**1000):
            with self.subTest(value=str(value)[:30]):
                control, candidate = records("core-proof")
                candidate["measurement"]["wall_seconds"] = value
                with self.assertRaises(RecordError):
                    compare(control, candidate)

    def test_incorrect_and_missing_evidence_rejected(self):
        for section, key, value in (
            ("correctness", "passed", False),
            ("correctness", "passed", 1),
            ("correctness", "report_sha256", ""),
            ("source", "revision", "main"),
            ("compatibility", "runtime_revision", "v6.1.0"),
            ("compatibility", "fixture_sha256", "0" * 64),
        ):
            with self.subTest(key=key, value=value):
                control, candidate = records()
                candidate[section][key] = value
                with self.assertRaises(RecordError):
                    compare(control, candidate)

    def test_same_run_or_raw_measurement_cannot_be_independent_control(self):
        for key in ("run_id", "measurement_artifact_sha256"):
            control, candidate = records()
            candidate[key] = control[key]
            with self.assertRaises(RecordError):
                compare(control, candidate)

    def test_imported_evidence_stays_imported_and_cannot_mix(self):
        control, candidate = records()
        candidate["evidence_origin"] = "imported"
        with self.assertRaisesRegex(RecordError, "cannot mix"):
            compare(control, candidate)
        control["evidence_origin"] = "imported"
        self.assertEqual(compare(control, candidate)["evidence_origin"], "imported")

    def test_template_is_never_measured_evidence(self):
        control, candidate = records()
        for flag in (True, False, None):
            candidate["template"] = flag
            with self.assertRaisesRegex(RecordError, "template"):
                compare(control, candidate)

    def test_json_duplicates_and_nonfinite_constants_rejected(self):
        for content in ('{"schema_version":1,"schema_version":2}', '{"cycles": NaN}'):
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "run.json"
                path.write_text(content)
                with self.assertRaises(RecordError):
                    read_record(path)

    def test_example_is_valid_json_but_cannot_be_compared(self):
        path = Path(__file__).with_name("run-record.example.json")
        record = json.loads(path.read_text())
        with self.assertRaisesRegex(RecordError, "template"):
            compare(record, records()[1])


if __name__ == "__main__":
    unittest.main()
