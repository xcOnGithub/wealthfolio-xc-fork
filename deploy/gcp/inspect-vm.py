#!/usr/bin/env python3
"""Read-only inventory. Never emit container environment values except this allowlist."""

import json
import subprocess

PUBLIC_SETTINGS = {
    "WF_DB_PATH", "WF_DATA_DIR", "WF_SECRET_KEY_FILE", "WF_RESEND_API_KEY_FILE",
    "WF_DB_REQUIRE_ENCRYPTION", "WF_ACCOUNT_AUTH", "WF_CORS_ALLOW_ORIGINS",
    "WF_AUTH_EMAIL_FROM",
}


def inventory():
    names = subprocess.check_output(["docker", "ps", "-aq"], text=True).split()
    if not names:
        return {"containers": []}
    records = json.loads(subprocess.check_output(["docker", "inspect", *names]))
    result = []
    for record in records:
        environment = dict(value.split("=", 1) for value in record["Config"].get("Env", []) if "=" in value)
        result.append({
            "name": record["Name"].lstrip("/"), "image": record["Config"]["Image"],
            "running": record["State"]["Running"],
            "project": (record["Config"].get("Labels") or {}).get("com.docker.compose.project"),
            "ports": record["HostConfig"].get("PortBindings", {}),
            "networks": list(record.get("NetworkSettings", {}).get("Networks", {})),
            "mounts": [{key: mount.get(key) for key in ("Type", "Name", "Source", "Destination", "RW")} for mount in record.get("Mounts", [])],
            "settings": {key: value for key, value in environment.items() if key in PUBLIC_SETTINGS},
            "credential_inputs_present": [key for key in ("WF_SECRET_KEY", "WF_SECRET_KEY_FILE", "WF_RESEND_API_KEY", "WF_RESEND_API_KEY_FILE", "WF_SIGNUP_INVITE_HASH") if environment.get(key)],
        })
    return {"containers": result}


if __name__ == "__main__":
    print(json.dumps(inventory(), indent=2))
