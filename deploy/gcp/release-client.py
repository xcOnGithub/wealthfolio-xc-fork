#!/usr/bin/env python3
"""Send only an image release request to the root-installed VM command over IAP."""
import argparse
import json
import os
import re
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["deploy", "rollback", "status"])
    parser.add_argument("--project", required=True)
    parser.add_argument("--instance", required=True)
    parser.add_argument("--zone", required=True)
    parser.add_argument("--image")
    parser.add_argument("--commit")
    parser.add_argument("--run-url")
    args = parser.parse_args()
    request = {"operation": args.operation}
    if args.operation != "status":
        if not args.image or not re.fullmatch(r"[a-z0-9-]+-docker\.pkg\.dev/[a-z0-9-]+/[a-z0-9/_-]+@sha256:[0-9a-f]{64}", args.image):
            parser.error("An immutable Artifact Registry digest is required")
        if not args.commit or not re.fullmatch(r"[0-9a-f]{40}", args.commit):
            parser.error("A full source commit is required")
        if not args.run_url or not re.fullmatch(r"https://github\.com/xcOnGithub/wealthfolio-private/actions/runs/[1-9][0-9]*(?:/attempts/[1-9][0-9]*)?", args.run_url):
            parser.error("A repository Actions run URL is required")
        request.update(image=args.image, commit=args.commit, run_url=args.run_url)
    command = ["gcloud", "compute", "ssh", args.instance, "--project", args.project, "--zone", args.zone,
               "--tunnel-through-iap", "--ssh-flag=-T", "--quiet", "--verbosity=error",
               "--command", "sudo /opt/wealthfolio/release.py"]
    if os.environ.get("WF_GCP_SSH_KEY_FILE"):
        command += ["--ssh-key-file", os.environ["WF_GCP_SSH_KEY_FILE"]]
    result = subprocess.run(command, input=json.dumps(request).encode(), capture_output=True)
    if result.returncode:
        print("VM release failed. Review the production service journal; no credential values are included in this report.", file=sys.stderr)
        return 1
    state = json.loads(result.stdout)
    print(json.dumps({"current": state["current"], "recovery_required": state.get("recovery_required", False)}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
