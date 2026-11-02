#!/usr/bin/env python3
"""Root-installed image-only release endpoint. Never restore application/account data."""
import datetime
import fcntl
import hmac
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("runtime", HERE / "fetch-secrets.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)
CONTRACT = "owner-auth-files-v1"
CONTAINER = "wealthfolio-wealthfolio-1"
RUN_URL = re.compile(r"https://github\.com/xcOnGithub/wealthfolio-xc-fork/actions/runs/[1-9][0-9]*(?:/attempts/[1-9][0-9]*)?\Z")


def command(arguments, env=None):
    result = subprocess.run(arguments, capture_output=True, env=env, timeout=600)
    if result.returncode:
        raise RuntimeError("Release operation failed: " + arguments[0])
    return result.stdout.decode()


def validate_request(request, config):
    if not isinstance(request, dict):
        raise ValueError("Release request must be an object")
    operation = request.get("operation")
    allowed = {"operation"} if operation == "status" else {"operation", "image", "commit", "run_url"}
    if operation not in {"status", "deploy", "rollback"} or set(request) != allowed:
        raise ValueError("Invalid release request")
    if operation == "status":
        return
    if not re.fullmatch(re.escape(config["image"].split("@")[0]) + r"@sha256:[0-9a-f]{64}", request["image"]):
        raise ValueError("Only this installation's immutable application image may change")
    if not re.fullmatch(r"[0-9a-f]{40}", request["commit"]) or not RUN_URL.fullmatch(request["run_url"]):
        raise ValueError("Invalid release provenance")


def image_metadata(image, run=command):
    labels = json.loads(run(["docker", "image", "inspect", "--format", "{{json .Config.Labels}}", image])) or {}
    result = {
        "image": image, "commit": labels.get("org.opencontainers.image.revision", ""),
        "fingerprint": labels.get("org.wealthfolio.state-fingerprint", ""),
        "contract": labels.get("org.wealthfolio.config-contract", ""),
    }
    if result["contract"] != CONTRACT or not re.fullmatch(r"[0-9a-f]{40}", result["commit"]) or not re.fullmatch(r"[0-9a-f]{64}", result["fingerprint"]):
        raise ValueError("Image is missing required release/compatibility labels")
    return result


def backup_data(data, destination):
    data, destination = Path(data).resolve(), Path(destination).resolve()
    if data == destination or data in destination.parents or not data.is_dir() or not any(data.iterdir()):
        raise ValueError("Existing data and a separate backup directory are required")
    import shutil
    size = sum(path.stat().st_size for path in data.rglob("*") if path.is_file() and not path.is_symlink())
    destination.mkdir(mode=0o700, parents=True, exist_ok=True)
    destination.chmod(0o700)
    if shutil.disk_usage(destination).free < size + 64 * 1024 * 1024:
        raise ValueError("Insufficient space for a consistent pre-release backup")
    descriptor, name = tempfile.mkstemp(suffix=".tar.gz", dir=destination)
    os.close(descriptor)
    try:
        with tarfile.open(name, "w:gz") as archive:
            archive.add(data, arcname="data", recursive=True)
    except BaseException:
        Path(name).unlink(missing_ok=True)
        raise
    return name


def verify(config, image, run=command):
    expected = run(["docker", "image", "inspect", "--format", "{{.Id}}", image]).strip()
    actual = run(["docker", "inspect", "--format", "{{.Image}}", CONTAINER]).strip()
    if not expected or expected != actual:
        raise ValueError("Running container does not match the selected image")
    for base in ["http://127.0.0.1:" + str(config["bind_port"]), "https://" + config["domain"]]:
        with urllib.request.urlopen(base + "/api/v1/healthz", timeout=10) as response:
            if response.status != 200:
                raise ValueError("Health check failed")
        with urllib.request.urlopen(base + "/api/v1/auth/status", timeout=10) as response:
            status = json.load(response)
        if status.get("accountAuth") is not True or status.get("requiresPassword") is not True:
            raise ValueError("Owner authentication is not required")
        try:
            urllib.request.urlopen(base + "/api/v1/auth/me", timeout=10).close()
        except urllib.error.HTTPError as error:
            if error.code != 401:
                raise ValueError("Unexpected anonymous authentication response") from None
        else:
            raise ValueError("Anonymous session was admitted")


class Releases:
    def __init__(self, config_path=runtime.CONFIG, state_dir=Path("/var/lib/wealthfolio-release"), run=command, health=None, prepare=None):
        self.config_path, self.state_dir = Path(config_path), Path(state_dir)
        self.run = run
        self.health = health or (lambda config, image: verify(config, image, run))
        self.prepare = prepare or self.prepare_image

    def config(self):
        return json.loads(self.config_path.read_text())

    def state(self):
        state = json.loads((self.state_dir / "history.json").read_text())
        if state["current"]["image"] != self.config()["image"] and not state.get("recovery_required"):
            raise ValueError("Release history and installed configuration differ; operator recovery is required")
        return state

    def save(self, state):
        self.state_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.state_dir.chmod(0o700)
        runtime.atomic_write(self.state_dir / "history.json", json.dumps(state).encode())

    def initialize(self):
        config = self.config()
        release = image_metadata(config["image"], self.run)
        self.wait_healthy(config, release["image"])
        path = self.state_dir / "history.json"
        if path.exists():
            state = self.state()
            if state["current"]["image"] != release["image"]:
                raise ValueError("Existing release history cannot be replaced by bootstrap")
            return state
        release.update(run_url="bootstrap", time=datetime.datetime.now(datetime.timezone.utc).isoformat())
        state = {"current": release, "successful": [release]}
        self.save(state)
        return state

    def prepare_image(self, config, image):
        # The host identity alone reads secrets. CI sends no secret/config payload.
        project = runtime.metadata("project/project-id")
        token = runtime.metadata("instance/service-accounts/default/token", json_value=True)["access_token"]
        with tempfile.TemporaryDirectory(prefix="wealthfolio-pull-", dir="/run") as temporary:
            runtime.load(config, project, token, Path(temporary))
            if not hmac.compare_digest((runtime.RUNTIME / "master-key").read_bytes().strip(), (Path(temporary) / "master-key").read_bytes().strip()):
                raise ValueError("The installation master key changed; refusing automatic cutover")
            environment = dict(os.environ, DOCKER_CONFIG=str(Path(temporary) / "docker"))
            self.run(["docker", "pull", image], env=environment)
        if config.get("data_volume"):
            volume = json.loads(self.run(["docker", "volume", "inspect", config["data_volume"]]))[0]
            if volume["Driver"] != "local" or volume.get("Options") or Path(volume["Mountpoint"]).resolve() != Path(config["data_dir"]).resolve():
                raise ValueError("Persistent volume no longer matches the installation")

    def wait_healthy(self, config, image):
        deadline = time.monotonic() + 180
        while True:
            try:
                self.health(config, image)
                return
            except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
                if time.monotonic() >= deadline:
                    raise RuntimeError("Application/HTTPS/authentication verification failed") from None
                time.sleep(2)

    def apply(self, request):
        config = self.config()
        validate_request(request, config)
        state = self.state()
        if request["operation"] == "status":
            return state
        if state.get("recovery_required"):
            raise ValueError("A previous deployment requires operator recovery")
        current = state["current"]
        image = request["image"]
        if image == current["image"]:
            self.health(config, image)
            return state
        self.prepare(config, image)
        candidate = image_metadata(image, self.run)
        if request["operation"] == "rollback":
            known = next((entry for entry in state["successful"] if entry["image"] == image), None)
            if known is None or any(known[key] != candidate[key] for key in ("commit", "fingerprint", "contract")):
                raise ValueError("Rollback requires a recorded successful release")
            if candidate["fingerprint"] != current["fingerprint"]:
                raise ValueError("Backend-state changes require a manual recovery decision")
        elif candidate["commit"] != request["commit"]:
            raise ValueError("Image source revision differs from the validated commit")
        candidate.update(run_url=request["run_url"], time=datetime.datetime.now(datetime.timezone.utc).isoformat())
        self.run(["systemctl", "stop", "wealthfolio.service"])
        try:
            backup = backup_data(config["data_dir"], self.state_dir / "backups")
        except BaseException:
            self.run(["systemctl", "start", "wealthfolio.service"])
            raise
        updated = dict(config, image=image)
        runtime.atomic_write(self.config_path, json.dumps(updated).encode())
        try:
            self.run(["systemctl", "start", "wealthfolio.service"])
            self.wait_healthy(updated, image)
        except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
            self.run(["systemctl", "stop", "wealthfolio.service"])
            if candidate["fingerprint"] == current["fingerprint"]:
                runtime.atomic_write(self.config_path, json.dumps(config).encode())
                self.run(["systemctl", "start", "wealthfolio.service"])
                self.wait_healthy(config, current["image"])
                state.setdefault("failed", []).append(dict(candidate, automatic_image_rollback=True, backup=backup))
                self.save(state)
                raise RuntimeError("Candidate failed verification; previous compatible image restored; application/account data was not restored") from None
            state["recovery_required"] = True
            state.setdefault("failed", []).append(dict(candidate, automatic_image_rollback=False, backup=backup))
            self.save(state)
            raise RuntimeError("Candidate failed with possible backend-state changes; service stopped, backup preserved, manual recovery required") from None
        candidate["backup"] = backup
        state["successful"].append(candidate)
        state["current"] = candidate
        self.save(state)
        return state


def main():
    os.umask(0o077)
    try:
        Path("/run/lock").mkdir(exist_ok=True)
        with open("/run/lock/wealthfolio-release.lock", "a") as lock:
            # Also serializes operator invocations; GitHub concurrency cannot do that.
            fcntl.flock(lock, fcntl.LOCK_EX)
            releases = Releases()
            if sys.argv[1:] == ["--initialize"]:
                state = releases.initialize()
            elif not sys.argv[1:]:
                source = sys.stdin.read(8193)
                if len(source) > 8192:
                    raise ValueError("Release request is too large")
                state = releases.apply(json.loads(source))
            else:
                raise ValueError("Unsupported release arguments")
            print(json.dumps(state))
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.TimeoutExpired) as error:
        message = str(error) if isinstance(error, RuntimeError) else "Inspect the installed configuration and recorded release state"
        print("Release failed: " + message + ". No data restore was performed.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
