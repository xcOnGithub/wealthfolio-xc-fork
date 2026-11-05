import unittest

from plaid_coverage import meets_threshold, summarize

SOURCE = "/repo/crates/connect/src/plaid/client.rs"


def report(lines=100, hits=95, branches=100, branch_hits=95):
    return f"SF:{SOURCE}\nLF:{lines}\nLH:{hits}\nBRF:{branches}\nBRH:{branch_hits}\nend_of_record\n"


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

    def test_missing_metric_is_rejected(self):
        with self.assertRaises(KeyError):
            summarize(report().replace("BRF:100\n", ""), {SOURCE})


if __name__ == "__main__":
    unittest.main()
