import unittest

from plaid_coverage import meets_threshold, summarize

SOURCE = "/repo/crates/connect/src/plaid/client.rs"


def report(lines=100, hits=95, branches=100, branch_hits=95):
    rows = "".join(f"DA:{n + 1},{int(n < hits)}\n" for n in range(lines))
    rows += "".join(f"BRDA:{n + 1},0,0,{int(n < branch_hits)}\n" for n in range(branches))
    return f"SF:{SOURCE}\n{rows}LF:{lines}\nLH:{hits}\nBRF:{branches}\nBRH:{branch_hits}\nend_of_record\n"


class PlaidCoverageTests(unittest.TestCase):
    def test_independent_thresholds_and_exact_boundary(self):
        for hits, branch_hits, expected in [(95, 95, True), (94, 100, False), (100, 94, False)]:
            self.assertEqual(meets_threshold(summarize(report(hits=hits, branch_hits=branch_hits), {SOURCE})), expected)

    def test_absent_instrumentation_is_not_a_pass(self):
        for data in ["", report(lines=0, hits=0), report(branches=0, branch_hits=0)]:
            with self.assertRaises(ValueError):
                summarize(data, {SOURCE})

    def test_unrelated_duplicate_and_invalid_metrics_fail_closed(self):
        for data, allowed in [(report(), set()), (report() * 2, {SOURCE}),
                              (report(hits=101), {SOURCE}), (report(branch_hits=-1), {SOURCE})]:
            with self.assertRaises(ValueError):
                summarize(data, allowed)

    def test_instantiations_do_not_inflate_source_line_totals(self):
        data = report().replace("LF:100\n", "LF:200\n")
        self.assertEqual(summarize(data, {SOURCE})["lines"], {"covered": 95, "total": 100, "percent": 95.0})

    def test_summary_without_actual_rows_is_not_instrumentation(self):
        data = "\n".join(line for line in report().splitlines() if not line.startswith("DA:"))
        with self.assertRaises(ValueError):
            summarize(data, {SOURCE})

    def test_missing_service_or_storage_export_is_rejected(self):
        for path in ["/repo/crates/connect/src/plaid/service.rs", "/repo/crates/storage-sqlite/src/sync/plaid.rs"]:
            with self.subTest(path=path), self.assertRaises(ValueError):
                summarize(report(), {SOURCE, path})

    def test_missing_metric_is_rejected(self):
        with self.assertRaises(KeyError):
            summarize(report().replace("BRF:100\n", ""), {SOURCE})


if __name__ == "__main__":
    unittest.main()
