import unittest
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
from unittest.mock import patch

import deployment_coverage
from deployment_coverage import check


class DeploymentCoverageTests(unittest.TestCase):
    def report(self, lines=95, branches=95):
        return {"meta": {"branch_coverage": True}, "files": {"deploy/gcp/example.py": {"summary": {
            "covered_lines": lines, "num_statements": 100,
            "covered_branches": branches, "num_branches": 100,
        }}}}

    def test_separate_line_and_branch_counts_are_preserved(self):
        result = check(self.report(lines=99, branches=94), {"deploy/gcp/example.py"})
        self.assertEqual(result["lines"]["percent"], 99)
        self.assertEqual(result["branches"]["percent"], 94)

    def test_cli_requires_both_thresholds_even_when_the_other_metric_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "deploy/gcp").mkdir(parents=True)
            (root / "deploy/gcp/example.py").write_text("pass\n")
            report = root / "report.json"
            for lines, branches, status in [(94, 100, 1), (100, 94, 1), (95, 95, 0), (100, 100, 0)]:
                report.write_text(json.dumps(self.report(lines, branches)))
                with self.subTest(lines=lines, branches=branches), patch.object(deployment_coverage, "ROOT", root), patch.object(sys, "argv", ["deployment_coverage.py", str(report)]), contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(deployment_coverage.main(), status)

    def test_missing_scripts_tests_in_scope_or_empty_instrumentation_fail_closed(self):
        report = self.report(); report["meta"]["branch_coverage"] = False
        with self.assertRaisesRegex(ValueError, "Branch instrumentation"):
            check(report, {"deploy/gcp/example.py"})
        for expected in [set(), {"deploy/gcp/missing.py"}, {"deploy/gcp/example.py", "deploy/gcp/test_example.py"}]:
            with self.subTest(expected=expected), self.assertRaises(ValueError):
                check(self.report(), expected)
        for field in ["num_statements", "num_branches"]:
            report = self.report(); report["files"]["deploy/gcp/example.py"]["summary"][field] = 0
            with self.subTest(field=field), self.assertRaises(ValueError):
                check(report, {"deploy/gcp/example.py"})
