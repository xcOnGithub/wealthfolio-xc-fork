"""Export only Plaid sources: LLVM 23 crashes exporting unrelated Rust mappings.

Like export_owner_auth_coverage.py, use explicit source paths and the binaries
actually executed. Require real line AND branch instrumentation, independently.
"""
import json
import os
from pathlib import Path
import re
import subprocess


def summarize(lcov, allowed_sources):
    totals = {"lines": {"covered": 0, "total": 0},
              "branches": {"covered": 0, "total": 0}}
    seen = set()
    for record in lcov.split("end_of_record"):
        values = dict(line.split(":", 1) for line in record.splitlines()
                      if line.startswith(("SF:", "LF:", "LH:", "BRF:", "BRH:")))
        if not values:
            continue
        source = values.get("SF")
        if source not in allowed_sources or source in seen:
            raise ValueError("Unexpected or duplicate source coverage")
        seen.add(source)
        for metric, found, hit in [("lines", "LF", "LH"), ("branches", "BRF", "BRH")]:
            total, covered = int(values[found]), int(values[hit])
            if not 0 <= covered <= total:
                raise ValueError("Invalid coverage counts")
            totals[metric]["covered"] += covered
            totals[metric]["total"] += total
    if not any(source.endswith("/client.rs") for source in seen):
        raise ValueError("Missing provider client coverage")
    for metric, counts in totals.items():
        if counts["total"] == 0:
            raise ValueError(f"Missing {metric} instrumentation")
        counts["percent"] = 100 * counts["covered"] / counts["total"]
    return totals


def meets_threshold(summary):
    return all(v["covered"] * 100 >= v["total"] * 95 for v in summary.values())


def main():
    root = Path(__file__).resolve().parents[2]
    output = root / "coverage/plaid"
    output.mkdir(parents=True, exist_ok=True)
    base = Path(os.environ.get("CARGO_TARGET_DIR", root / "target")) / "llvm-cov-target"
    base = base.resolve()
    profiles = sorted(base.rglob("*.profraw"))
    if not profiles:
        raise SystemExit("No fresh Plaid test profiles")
    log = (output / "tests.log").read_text()
    objects = list(dict.fromkeys(
        (root / name).resolve() for name in re.findall(r"Running [^\n]* \(([^)]+)\)", log)
    ))
    if not objects or any(not p.is_relative_to(base) or not p.is_file() for p in objects):
        raise SystemExit("Missing executed coverage test binaries")
    toolchain = "+" + os.environ.get("WF_COVERAGE_TOOLCHAIN", "nightly-2026-11-01")
    sysroot = subprocess.check_output(["rustc", toolchain, "--print", "sysroot"], text=True).strip()
    version = subprocess.check_output(["rustc", toolchain, "-vV"], text=True)
    host = next(line.split(": ", 1)[1] for line in version.splitlines() if line.startswith("host: "))
    tools = Path(sysroot) / "lib/rustlib" / host / "bin"
    profile = base / "plaid.profdata"
    subprocess.run([str(tools / "llvm-profdata"), "merge", "--sparse", "-o", str(profile),
                    *map(str, profiles)], check=True)
    sources = sorted(p.resolve() for p in (root / "crates/connect/src/plaid").rglob("*.rs")
                     if p.name != "tests.rs" and not p.name.endswith("_tests.rs"))
    args = [str(tools / "llvm-cov"), "export", "-format=lcov",
            f"-instr-profile={profile}", str(objects[0])]
    for obj in objects[1:]:
        args += ["-object", str(obj)]
    args += list(map(str, sources))
    with (output / "coverage.lcov").open("w") as out, (output / "llvm-export.log").open("w") as err:
        subprocess.run(args, cwd=root, stdout=out, stderr=err, check=True)
    summary = summarize((output / "coverage.lcov").read_text(), set(map(str, sources)))
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))
    if not meets_threshold(summary):
        raise SystemExit("Plaid line or branch coverage below 95 percent")


if __name__ == "__main__":
    main()
