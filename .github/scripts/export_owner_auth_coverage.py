"""Export only auth/config files; all-source LLVM export crashes on unrelated code."""
import os
from pathlib import Path
import re
import subprocess

root = Path(__file__).resolve().parents[2]
base = root / "target/llvm-cov-target"
toolchain = "+" + os.environ.get("WF_COVERAGE_TOOLCHAIN", "nightly-2026-11-01")
sysroot = subprocess.check_output(["rustc", toolchain, "--print", "sysroot"], text=True).strip()
version = subprocess.check_output(["rustc", toolchain, "-vV"], text=True)
host = next(line.split(": ", 1)[1] for line in version.splitlines() if line.startswith("host: "))
tools = Path(sysroot) / "lib/rustlib" / host / "bin"
profiles = list(base.rglob("*.profraw"))
if not profiles:
    raise SystemExit("No fresh server coverage profiles found")
subprocess.run([
    str(tools / "llvm-profdata"), "merge", "--sparse", "-o",
    str(base / "wealthfolio-private.profdata"), *map(str, profiles),
], check=True)
log = (root / "coverage/owner-auth/server-tests.log").read_text()
objects = list(dict.fromkeys(
    root / name for name in re.findall(r"Running [^\n]* \((target/[^)]+)\)", log)
))
main = base / "debug/wealthfolio-server"
if main.exists():
    objects.append(main)
if not objects:
    raise SystemExit("No server test binaries found in the completed coverage run")
args = [str(tools / "llvm-cov"), "export", "-format=lcov",
        f"-instr-profile={base}/wealthfolio-private.profdata", str(objects[0])]
for obj in objects[1:]:
    args += ["-object", str(obj)]
args += [str(root / "apps/server/src" / f"{name}.rs") for name in ["account_auth", "auth", "config"]]
with open(root / "coverage/owner-auth/server.lcov", "w") as out, open(root / "coverage/owner-auth/llvm-export.log", "w") as err:
    subprocess.run(args, cwd=root, stdout=out, stderr=err, check=True)
print(f"Exported {len(profiles)} profiles from {len(objects)} binaries")
