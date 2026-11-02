#!/usr/bin/env python3
"""Pipe selected installation credentials to the deployment controller, never to a log."""

import json
import re
import subprocess
import sys


def export(container):
    if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]*", container):
        raise ValueError("Invalid container name")
    record = json.loads(subprocess.check_output(["docker", "inspect", container]))[0]
    if "wealthfolio" not in record["Config"]["Image"].lower():
        raise ValueError("Selected container is not a Wealthfolio image")
    environment = dict(value.split("=", 1) for value in record["Config"].get("Env", []) if "=" in value)
    result = {}
    for key in ("WF_SECRET_KEY", "WF_RESEND_API_KEY"):
        inline = environment.get(key, "").strip()
        file = environment.get(key + "_FILE", "")
        if inline and file:
            raise ValueError("Ambiguous credential inputs")
        if file:
            inline = subprocess.check_output(["docker", "exec", "--user", "0", container, "cat", file], stderr=subprocess.DEVNULL).decode("utf-8").strip()
        if inline:
            result[key] = inline
    if environment.get("WF_SIGNUP_INVITE_HASH"):
        result["WF_SIGNUP_INVITE_HASH"] = environment["WF_SIGNUP_INVITE_HASH"]
    if not result.get("WF_SECRET_KEY"):
        raise ValueError("Existing installation's master key is unavailable")
    return result


if __name__ == "__main__":
    try:
        if sys.stdout.isatty():
            raise ValueError("Credential export requires a private pipe, not a terminal")
        print(json.dumps(export(sys.argv[-1])))
    except (ValueError, OSError, subprocess.CalledProcessError, KeyError, IndexError):
        print("Could not export the selected installation's credentials", file=sys.stderr)
        sys.exit(1)
