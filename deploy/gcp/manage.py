#!/usr/bin/env python3
"""Operator commands for an existing Wealthfolio VM. Secret values use stdin only."""

import argparse
import base64
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import socket
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SECRET_IDS = {"master_key": "wealthfolio-master-key", "resend_api_key": "wealthfolio-resend-api-key", "invitation_hash": "wealthfolio-invitation-hash"}
spec = importlib.util.spec_from_file_location("runtime_secrets", HERE / "fetch-secrets.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class DeploymentError(Exception):
    pass


def run(arguments, *, data=None, optional=False):
    result = subprocess.run(arguments, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        # Neither stdout nor stderr is echoed: credential-export output and SDK
        # diagnostics must not accidentally enter deployment logs.
        if optional and b"NOT_FOUND" in result.stderr:
            return None
        raise DeploymentError("Command failed: " + shlex.join(arguments[:3]) + "; inspect permissions and the selected resource")
    return result.stdout


def gcloud(arguments, project, *, data=None, optional=False):
    return run(["gcloud", *arguments, "--project", project, "--quiet", "--format=json", "--verbosity=error"], data=data, optional=optional)


def resource(arguments, project, *, optional=False):
    result = gcloud(arguments, project, optional=optional)
    return json.loads(result) if result is not None else None


def remote_script(project, zone, instance, filename, arguments=()):
    code = base64.b64encode((HERE / filename).read_bytes()).decode()
    command = shlex.join(["sudo", "python3", "-c", "import base64; exec(base64.b64decode(" + repr(code) + "))", *arguments])
    return ssh(project, zone, instance, command)


def ssh(project, zone, instance, command):
    arguments = ["compute", "ssh", instance, "--zone", zone, "--tunnel-through-iap", "--ssh-flag=-T", "--command", command]
    if os.environ.get("WF_GCP_SSH_KEY_FILE"):
        arguments += ["--ssh-key-file", os.environ["WF_GCP_SSH_KEY_FILE"]]
    return gcloud(arguments, project)


def inspect(args):
    vm = resource(["compute", "instances", "describe", args.instance, "--zone", args.zone], args.project)
    # Instance metadata can contain old inline credentials; only names are emitted.
    output = {key: vm.get(key) for key in ("name", "status", "machineType", "disks", "networkInterfaces", "serviceAccounts")}
    output["metadata_keys"] = [item["key"] for item in vm.get("metadata", {}).get("items", [])]
    output["docker"] = json.loads(remote_script(args.project, args.zone, args.instance, "inspect-vm.py"))
    print(json.dumps(output, indent=2))


def read_env(path):
    """Read credential assignments without executing or interpolating dotenv text."""
    allowed = {"WF_SECRET_KEY", "WF_RESEND_API_KEY", "WF_SIGNUP_INVITE_HASH"}
    values = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        name, separator, value = line.partition("=")
        name = name.removeprefix("export ").strip()
        if not separator or name not in allowed:
            continue
        value = value.strip()
        if value.startswith('"'):
            value = json.loads(value)
        elif value.startswith("'"):
            if not value.endswith("'"):
                raise DeploymentError("Unterminated credential value")
            value = value[1:-1]
        if not isinstance(value, str) or any(character in value for character in "\r\n\0"):
            raise DeploymentError("Invalid credential assignment")
        values[name] = value
    return values


def enabled_version(project, secret):
    record = resource(["secrets", "describe", secret], project, optional=True)
    if record is None:
        gcloud(["secrets", "create", secret, "--replication-policy=automatic"], project)
        return None
    versions = resource(["secrets", "versions", "list", secret, "--filter=state:ENABLED", "--sort-by=~createTime", "--limit=1"], project)
    return "projects/" + project + "/secrets/" + secret + "/versions/" + versions[0]["name"].rsplit("/", 1)[1] if versions else None


def seed(args):
    gcloud(["services", "enable", "secretmanager.googleapis.com"], args.project)
    values = read_env(args.env_file) if args.env_file else {}
    if args.instance:
        if not args.container:
            raise DeploymentError("Select the existing application container before importing its key")
        # Captured only in memory. No exported JSON or secret value is printed.
        existing = json.loads(remote_script(args.project, args.zone, args.instance, "export-credentials.py", [args.container]))
        values.update(existing)
    if not values.get("WF_SECRET_KEY"):
        raise DeploymentError("The existing installation's master key is required; no replacement key is generated")
    if not values.get("WF_RESEND_API_KEY"):
        raise DeploymentError("Supply the existing Resend key or an env file containing it")
    if not values.get("WF_SIGNUP_INVITE_HASH"):
        raise DeploymentError("Generate an invitation outside Git and supply its digest; never upload its plaintext")
    if not re.fullmatch(r"[0-9a-fA-F]{64}", values["WF_SIGNUP_INVITE_HASH"]):
        raise DeploymentError("Invalid invitation digest")
    sources = {"master_key": "WF_SECRET_KEY", "resend_api_key": "WF_RESEND_API_KEY", "invitation_hash": "WF_SIGNUP_INVITE_HASH"}
    versions = {}
    for key, secret in SECRET_IDS.items():
        current = enabled_version(args.project, secret)
        payload = values[sources[key]].encode()
        if current:
            previous = gcloud(["secrets", "versions", "access", current.rsplit("/", 1)[1], "--secret", secret], args.project)
            if previous.strip() != payload.strip():
                raise DeploymentError("Existing secret differs from the selected installation: " + secret + "; explicit rotation is required")
            versions[key] = current
        else:
            result = gcloud(["secrets", "versions", "add", secret, "--data-file=-"], args.project, data=payload)
            number = json.loads(result)["name"].rsplit("/", 1)[1]
            versions[key] = "projects/" + args.project + "/secrets/" + secret + "/versions/" + number
    # Resource names only; safe to use in a deployment plan.
    print(json.dumps(versions, indent=2))


def build(args):
    status = run(["git", "status", "--porcelain", "--untracked-files=all"], data=None)
    if status.strip():
        raise DeploymentError("Commit the exact source before building an immutable image")
    commit = run(["git", "rev-parse", "HEAD"]).decode().strip()
    fingerprint = run([sys.executable, str(HERE / "release-metadata.py")]).decode().strip()
    region = args.region
    gcloud(["services", "enable", "artifactregistry.googleapis.com", "cloudbuild.googleapis.com", "iam.googleapis.com"], args.project)
    repository = resource(["artifacts", "repositories", "describe", "wealthfolio", "--location", region], args.project, optional=True)
    if repository is None:
        gcloud(["artifacts", "repositories", "create", "wealthfolio", "--repository-format=docker", "--location", region, "--immutable-tags"], args.project)
    builder = "wealthfolio-builder@" + args.project + ".iam.gserviceaccount.com"
    if resource(["iam", "service-accounts", "describe", builder], args.project, optional=True) is None:
        gcloud(["iam", "service-accounts", "create", "wealthfolio-builder"], args.project)
    gcloud(["artifacts", "repositories", "add-iam-policy-binding", "wealthfolio", "--location", region, "--member=serviceAccount:" + builder, "--role=roles/artifactregistry.writer"], args.project)
    gcloud(["projects", "add-iam-policy-binding", args.project, "--member=serviceAccount:" + builder, "--role=roles/logging.logWriter", "--condition=None"], args.project)
    bucket = "gs://" + args.project + "-wealthfolio-build-source"
    if resource(["storage", "buckets", "describe", bucket], args.project, optional=True) is None:
        gcloud(["storage", "buckets", "create", bucket, "--location", region, "--uniform-bucket-level-access", "--public-access-prevention"], args.project)
    gcloud(["storage", "buckets", "add-iam-policy-binding", bucket, "--member=serviceAccount:" + builder, "--role=roles/storage.objectViewer"], args.project)
    image = region + "-docker.pkg.dev/" + args.project + "/wealthfolio/server:" + commit
    image_record = resource(["artifacts", "docker", "images", "describe", image], args.project, optional=True)
    if image_record is None:
        gcloud(["builds", "submit", str(ROOT), "--config", str(HERE / "cloudbuild.yaml"), "--region", region,
                "--service-account=projects/" + args.project + "/serviceAccounts/" + builder,
                "--gcs-source-staging-dir=" + bucket + "/source",
                "--substitutions=_IMAGE=" + image + ",_COMMIT=" + commit + ",_STATE_FINGERPRINT=" + fingerprint], args.project)
        image_record = resource(["artifacts", "docker", "images", "describe", image], args.project)
    print(image.split(":")[0] + "@" + image_record["image_summary"]["digest"])


def install(args):
    config = json.loads(args.config.read_text())
    runtime.validate_config(config, args.project)
    vm = resource(["compute", "instances", "describe", args.instance, "--zone", args.zone], args.project)
    keys = {item["key"] for item in vm.get("metadata", {}).get("items", [])}
    if keys & {"startup-script", "startup-script-url", "gce-container-declaration"}:
        raise DeploymentError("Existing startup automation must be audited and migrated before cutover")
    accounts = vm.get("serviceAccounts", [])
    if len(accounts) != 1 or "https://www.googleapis.com/auth/cloud-platform" not in accounts[0].get("scopes", []):
        raise DeploymentError("Inspect the existing VM service account and scopes before enabling Secret Manager")
    account = accounts[0]["email"]
    if config["manage_proxy"]:
        addresses = [access.get("natIP") for interface in vm.get("networkInterfaces", []) for access in interface.get("accessConfigs", []) if access.get("natIP")]
        resolved = {record[4][0] for record in socket.getaddrinfo(config["domain"], 443, socket.AF_INET)}
        if len(addresses) != 1 or addresses[0] not in resolved:
            raise DeploymentError("The domain's DNS A record must point directly to this VM before enabling its HTTPS proxy")
    for version in config["secrets"].values():
        secret = version.split("/")[3]
        gcloud(["secrets", "add-iam-policy-binding", secret, "--member=serviceAccount:" + account, "--role=roles/secretmanager.secretAccessor"], args.project)
    location = config["registry"].removesuffix("-docker.pkg.dev")
    repository = config["image"].split("/")[2]
    gcloud(["artifacts", "repositories", "add-iam-policy-binding", repository, "--location", location, "--member=serviceAccount:" + account, "--role=roles/artifactregistry.reader"], args.project)
    if config["manage_proxy"]:
        network = vm["networkInterfaces"][0]["network"]
        if "/projects/" + args.project + "/" not in network:
            raise DeploymentError("Shared-VPC firewall changes require a separate network-owner review")
        tag = "wf-web-" + str(vm["id"])
        if resource(["compute", "firewall-rules", "describe", tag], args.project, optional=True) is None:
            gcloud(["compute", "firewall-rules", "create", tag, "--network", network, "--target-tags", tag, "--allow=tcp:80,tcp:443", "--source-ranges=0.0.0.0/0"], args.project)
        gcloud(["compute", "instances", "add-tags", args.instance, "--zone", args.zone, "--tags", tag], args.project)
    files = {name: base64.b64encode((HERE / name).read_bytes()).decode() for name in
             ("compose.yaml", "Caddyfile", "fetch-secrets.py", "release.py", "block-metadata.sh", "wealthfolio.service", "bootstrap.sh")}
    with tempfile.TemporaryDirectory(prefix="wealthfolio-metadata-") as temporary:
        path = Path(temporary)
        (path / "config.json").write_text(json.dumps(config))
        (path / "files.json").write_text(json.dumps(files))
        gcloud(["compute", "instances", "add-metadata", args.instance, "--zone", args.zone,
                "--metadata-from-file=wealthfolio-config=" + str(path / "config.json") + ",wealthfolio-files=" + str(path / "files.json")], args.project)
    command = "sudo python3 -c " + shlex.quote("import json,base64,urllib.request,subprocess; r=urllib.request.Request('http://metadata.google.internal/computeMetadata/v1/instance/attributes/wealthfolio-files',headers={'Metadata-Flavor':'Google'}); files=json.load(urllib.request.urlopen(r)); subprocess.run(['bash','-s'],input=base64.b64decode(files['bootstrap.sh']),check=True)")
    output = ssh(args.project, args.zone, args.instance, command)
    if b"WEALTHFOLIO_BOOTSTRAP_READY" not in output:
        raise DeploymentError("Deployment did not report a healthy application; inspect the VM service journal")
    print("Application health check passed. Verify HTTPS and account sign-in at https://" + config["domain"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True)
    sub = parser.add_subparsers(dest="operation", required=True)
    for name in ("inspect", "seed", "install"):
        command = sub.add_parser(name)
        command.add_argument("--zone", default="us-west1-b")
        command.add_argument("--instance", required=name != "seed")
        if name == "seed":
            command.add_argument("--container")
            command.add_argument("--env-file", type=Path)
        if name == "install":
            command.add_argument("--config", type=Path, required=True)
    command = sub.add_parser("build")
    command.add_argument("--region", default="us-west1")
    args = parser.parse_args()
    if not runtime.PROJECT.fullmatch(args.project):
        parser.error("Invalid project ID")
    if getattr(args, "instance", None) and not re.fullmatch(r"[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?", args.instance):
        parser.error("Invalid instance name")
    os.chdir(ROOT)
    try:
        {"inspect": inspect, "seed": seed, "build": build, "install": install}[args.operation](args)
    except (DeploymentError, ValueError, OSError, KeyError) as error:
        print("Deployment stopped: " + (str(error) if isinstance(error, DeploymentError) else "invalid configuration or unavailable dependency"), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
