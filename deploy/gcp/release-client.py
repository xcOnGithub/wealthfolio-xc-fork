#!/usr/bin/env python3
"""Send only an image release request to the root-installed VM command over IAP."""
import argparse
import json
import os
import re
import subprocess
import sys

IMAGE = re.compile(r"[a-z0-9-]+-docker\.pkg\.dev/[a-z0-9-]+/[a-z0-9/_-]+@sha256:[0-9a-f]{64}")
RESPONSE_PREFIX = "WEALTHFOLIO_RELEASE_JSON="


def parse_response(output, expected_image=None):
    # gcloud key generation and SSH startup banners may also write to stdout.
    prefix = RESPONSE_PREFIX.encode()
    records = [line[len(prefix):] for line in output.splitlines() if line.startswith(prefix)]
    if len(records) != 1:
        raise ValueError("Expected one release response")
    state = json.loads(records[0])
    current = state.get("current") if isinstance(state, dict) else None
    if not isinstance(current, dict) or not isinstance(current.get("image"), str):
        raise ValueError("Invalid current release")
    if not IMAGE.fullmatch(current["image"]):
        raise ValueError("Invalid current image")
    recovery = state.get("recovery_required", False)
    if not isinstance(recovery, bool) or (expected_image is not None and current["image"] != expected_image):
        raise ValueError("Release response does not match the request")
    return {"current": current, "recovery_required": recovery}


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
        if not args.image or not IMAGE.fullmatch(args.image):
            parser.error("An immutable Artifact Registry digest is required")
        if not args.commit or not re.fullmatch(r"[0-9a-f]{40}", args.commit):
            parser.error("A full source commit is required")
        if not args.run_url or not re.fullmatch(r"https://github\.com/xcOnGithub/wealthfolio-xc-fork/actions/runs/[1-9][0-9]*(?:/attempts/[1-9][0-9]*)?", args.run_url):
            parser.error("A repository Actions run URL is required")
        request.update(image=args.image, commit=args.commit, run_url=args.run_url)
    command = ["gcloud", "compute", "ssh", args.instance, "--project", args.project, "--zone", args.zone,
               "--tunnel-through-iap", "--ssh-flag=-T", "--quiet", "--verbosity=error",
               "--command", f"printf '{RESPONSE_PREFIX}'; sudo -n /opt/wealthfolio/release.py"]
    if os.environ.get("WF_GCP_SSH_KEY_FILE"):
        command += ["--ssh-key-file", os.environ["WF_GCP_SSH_KEY_FILE"]]
    result = subprocess.run(command, input=json.dumps(request).encode(), capture_output=True)
    if result.returncode:
        print("VM release failed. Review the production service journal; no credential values are included in this report.", file=sys.stderr)
        return 1
    try:
        state = parse_response(result.stdout, args.image if args.operation != "status" else None)
    except ValueError:
        print("Invalid VM release response. The deployment may already have completed; verify VM release status before retrying. Captured output is omitted to protect credentials.", file=sys.stderr)
        return 1
    print(json.dumps(state, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
