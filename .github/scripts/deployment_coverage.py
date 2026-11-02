"""Enforce separate combined line/branch thresholds on all deployment Python."""
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
THRESHOLD = 95


def check(report, expected_files):
    if report.get("meta", {}).get("branch_coverage") is not True:
        raise ValueError("Branch instrumentation is required")
    files = report["files"]
    if set(files) != set(expected_files):
        raise ValueError("Coverage must include every deployment Python script and exclude tests only")
    if any(file["summary"]["num_statements"] <= 0 for file in files.values()):
        raise ValueError("Every deployment script must have line instrumentation")
    metrics = {}
    for name, covered_key, total_key in [
        ("lines", "covered_lines", "num_statements"),
        ("branches", "covered_branches", "num_branches"),
    ]:
        covered = sum(file["summary"][covered_key] for file in files.values())
        total = sum(file["summary"][total_key] for file in files.values())
        if total <= 0 or not 0 <= covered <= total:
            raise ValueError("Missing or invalid " + name + " instrumentation")
        metrics[name] = {"covered": covered, "total": total, "percent": 100 * covered / total}
    return metrics


def main():
    expected = {
        path.relative_to(ROOT).as_posix()
        for path in (ROOT / "deploy/gcp").glob("*.py")
        if not path.name.startswith("test_")
    }
    metrics = check(json.loads(Path(sys.argv[1]).read_text()), expected)
    for name, metric in metrics.items():
        print(f"Deployment {name}: {metric['covered']}/{metric['total']} ({metric['percent']:.2f}%; required {THRESHOLD}%)")
    return int(any(metric["covered"] * 100 < THRESHOLD * metric["total"] for metric in metrics.values()))


if __name__ == "__main__":
    sys.exit(main())
