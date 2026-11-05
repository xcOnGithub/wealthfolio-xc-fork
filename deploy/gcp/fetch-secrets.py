#!/usr/bin/env python3
"""Load pinned Secret Manager versions into root-controlled tmpfs at service start."""

import base64
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import urllib.error
import urllib.request

METADATA = "http://metadata.google.internal/computeMetadata/v1/"
RUNTIME = Path("/run/wealthfolio")
CONFIG = Path("/etc/wealthfolio/config.json")
PROJECT = re.compile(r"[a-z][a-z0-9-]{4,28}[a-z0-9]\Z")
DOMAIN = re.compile(r"(?=.{1,253}\Z)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}\Z")
REGISTRY = re.compile(r"[a-z0-9-]+-docker\.pkg\.dev\Z")


def http_json(url, headers):
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def metadata(path, *, json_value=False):
    request = urllib.request.Request(METADATA + path, headers={"Metadata-Flavor": "Google"})
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.load(response) if json_value else response.read().decode("utf-8")


def crc32c(data):
    result = 0xFFFFFFFF
    for byte in data:
        result ^= byte
        for _ in range(8):
            result = (result >> 1) ^ (0x82F63B78 if result & 1 else 0)
    return result ^ 0xFFFFFFFF


def validate_config(config, actual_project):
    project = config.get("project", "")
    if not PROJECT.fullmatch(project) or project != actual_project:
        raise ValueError("Deployment project must match this VM's project")
    if not DOMAIN.fullmatch(config.get("domain", "")):
        raise ValueError("A valid public DNS hostname is required")
    registry = config.get("registry", "")
    if not REGISTRY.fullmatch(registry):
        raise ValueError("Invalid Artifact Registry hostname")
    image = config.get("image", "")
    if not re.fullmatch(re.escape(registry + "/" + project + "/") + r"[a-z0-9/_-]+@sha256:[0-9a-f]{64}", image):
        raise ValueError("Application image must be a digest in the selected project's registry")
    if not re.fullmatch(r"(?:docker\.io/library/)?caddy@sha256:[0-9a-f]{64}", config.get("proxy_image", "")):
        raise ValueError("Caddy must be pinned to an official image digest")
    sender = config.get("email_from", "")
    if not sender.strip() or any(character in sender for character in "\r\n\0"):
        raise ValueError("Invalid recovery email sender")
    if not isinstance(config.get("require_db_encryption"), bool):
        raise ValueError("Database encryption requirement must be explicitly configured")
    if not isinstance(config.get("manage_proxy"), bool):
        raise ValueError("Reverse-proxy ownership must be explicitly configured")
    if config.get("dedicated_vm") is not True:
        raise ValueError("This deployment requires a dedicated VM; shared workloads need a separate metadata-isolation plan")
    if not re.fullmatch(r"/(?:[a-zA-Z0-9_.-]+/)+[a-zA-Z0-9_.-]+", config.get("data_dir", "")):
        raise ValueError("An absolute persistent data directory without whitespace is required")
    if any(part in {".", ".."} for part in Path(config["data_dir"]).parts):
        raise ValueError("Data directory cannot contain parent or current-directory components")
    if config["data_dir"] in {"/var/lib", "/var/lib/docker", "/var/lib/docker/volumes", "/var/backups", "/opt/wealthfolio", "/srv/wealthfolio"}:
        raise ValueError("Data directory must identify the installation, not a system directory")
    if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]*", config.get("previous_container", "")):
        raise ValueError("The existing application container must be identified")
    if not re.fullmatch(r"[a-zA-Z0-9_-][a-zA-Z0-9_.-]*", config.get("db_filename", "")):
        raise ValueError("A database filename without directory components is required")
    if config.get("data_volume") is not None and not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]*", config["data_volume"]):
        raise ValueError("Invalid existing Docker volume name")
    if not isinstance(config.get("bind_port"), int) or not 1024 <= config["bind_port"] <= 65535:
        raise ValueError("Invalid loopback application port")
    plaid = config.get("plaid", {})
    if not isinstance(plaid, dict) or not isinstance(plaid.get("enabled", False), bool):
        raise ValueError("Plaid enablement must be an explicit boolean")
    enabled = plaid.get("enabled", False)
    if enabled:
        if not config["require_db_encryption"]:
            raise ValueError("Direct Plaid requires encrypted profile databases")
        if not isinstance(plaid.get("environment"), str) or plaid["environment"] not in {"sandbox", "production"}:
            raise ValueError("Invalid Plaid environment")
        if not isinstance(plaid.get("client_id"), str) or not re.fullmatch(r"[a-zA-Z0-9_-]{1,128}", plaid["client_id"]):
            raise ValueError("Invalid Plaid client ID")
        if plaid.get("redirect_uri") != "https://" + config["domain"] + "/settings/accounts":
            raise ValueError("Plaid redirect must use this application's HTTPS accounts page")
        institutions = plaid.get("institutions", {})
        allowed = {"chase", "boa", "usbank", "amex", "merrill_edge", "schwab", "robinhood"}
        if not isinstance(institutions, dict) or not institutions or set(institutions) - allowed:
            raise ValueError("Configure supported, privately verified Plaid institution IDs")
        if any(not isinstance(value, str) or not re.fullmatch(r"ins_[0-9]+", value) for value in institutions.values()):
            raise ValueError("Invalid Plaid institution ID")
    secrets = config.get("secrets", {})
    required = {"master_key", "resend_api_key", "invitation_hash"}
    if not isinstance(secrets, dict) or required - set(secrets) or set(secrets) - required - {"plaid_secret"} or (enabled and "plaid_secret" not in secrets):
        raise ValueError("Required installation/Plaid secret references are missing or unexpected")
    for key, name in secrets.items():
        if key == "plaid_secret" and not enabled:
            continue
        if not isinstance(name, str) or not re.fullmatch(re.escape("projects/" + project + "/secrets/") + r"[a-zA-Z0-9_-]+/versions/[1-9][0-9]*", name):
            raise ValueError("Secret versions must be pinned numeric versions in this project")


def read_secret(name, token):
    value = http_json("https://secretmanager.googleapis.com/v1/" + name + ":access", {"Authorization": "Bearer " + token})
    payload = value["payload"]
    data = base64.b64decode(payload["data"], validate=True)
    if crc32c(data) != int(payload["dataCrc32c"]):
        raise ValueError("Secret Manager payload checksum mismatch")
    return data


def atomic_write(path, data, *, uid=0, mode=0o600):
    descriptor, temporary = tempfile.mkstemp(prefix=".secret-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            os.fchmod(stream.fileno(), mode)
            os.fchown(stream.fileno(), uid, uid)
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def compose_literal(value):
    # Compose interpolates YAML/JSON string values. Escape dollars before serialization.
    return value.replace("$", "$$")


def load(config, project, token, runtime=RUNTIME):
    validate_config(config, project)
    if not Path(config["data_dir"]).is_dir():
        raise ValueError("The persistent data directory is unavailable; refusing to create a fresh installation")
    # Complete and validate every response before replacing any credential files.
    enabled = config.get("plaid", {}).get("enabled", False)
    values = {key: read_secret(name, token) for key, name in config["secrets"].items() if key != "plaid_secret" or enabled}
    plaid_secret = None
    if enabled:
        plaid_secret = values["plaid_secret"].decode("ascii").strip()
        if not plaid_secret or len(plaid_secret) > 4096 or any(c.isspace() or ord(c) < 32 or ord(c) == 127 for c in plaid_secret):
            raise ValueError("Invalid Plaid secret payload")
    master = values["master_key"]
    if not master.strip() or len(master) > 4096:
        raise ValueError("Invalid master key payload")
    master.decode("utf-8")
    resend = values["resend_api_key"].decode("utf-8").strip()
    if not resend or len(resend) > 4096 or any(character.isspace() or ord(character) < 32 for character in resend):
        raise ValueError("Invalid Resend key payload")
    invitation = values["invitation_hash"].decode("ascii").strip()
    if not re.fullmatch(r"[0-9a-fA-F]{64}", invitation):
        raise ValueError("Invalid invitation digest payload")
    override = {"services": {"wealthfolio": {"environment": {
        "WF_SIGNUP_INVITE_HASH": invitation.lower(),
    }}}}
    # Compose's --env-file also interpolates values. Single-quoted values preserve
    # dollars; JSON overrides handle the sender so no env-file quoting is needed.
    override["services"]["wealthfolio"]["environment"]["WF_AUTH_EMAIL_FROM"] = compose_literal(config["email_from"])
    if config.get("data_volume"):
        override["volumes"] = {"existing_data": {"external": True, "name": config["data_volume"]}}
        override["services"]["wealthfolio"]["volumes"] = [{"type": "volume", "source": "existing_data", "target": "/data"}]
    if "plaid" in config:
        override["services"]["wealthfolio"]["environment"]["WF_PLAID_ENABLED"] = str(enabled).lower()
    if enabled:
        plaid = config["plaid"]
        override["services"]["wealthfolio"]["environment"].update({
            "WF_PLAID_ENV": plaid["environment"], "WF_PLAID_CLIENT_ID": plaid["client_id"],
            "WF_PLAID_REDIRECT_URI": plaid["redirect_uri"],
            "WF_PLAID_INSTITUTIONS": json.dumps(plaid["institutions"], separators=(",", ":")),
            "WF_PLAID_SECRET_FILE": "/run/secrets/plaid-secret",
        })
        override["services"]["wealthfolio"].setdefault("volumes", []).append({
            "type": "bind", "source": str(runtime / "plaid-secret"), "target": "/run/secrets/plaid-secret", "read_only": True,
            "bind": {"create_host_path": False},
        })
    environment = {
        "WF_IMAGE": config["image"], "WF_PROXY_IMAGE": config["proxy_image"],
        "WF_DOMAIN": config["domain"], "WF_AUTH_EMAIL_FROM": "configured-in-override",
        "WF_DB_REQUIRE_ENCRYPTION": str(config["require_db_encryption"]).lower(),
        "WF_APP_DATA_DIR": config["data_dir"], "WF_DB_FILENAME": config["db_filename"],
        "WF_BIND_PORT": str(config["bind_port"]),
        "COMPOSE_PROFILES": "proxy" if config["manage_proxy"] else "",
    }
    runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    runtime.chmod(0o700)
    docker = runtime / "docker"
    docker.mkdir(mode=0o700, exist_ok=True)
    auth = base64.b64encode(("oauth2accesstoken:" + token).encode()).decode()
    atomic_write(runtime / "master-key", master, uid=1000, mode=0o400)
    atomic_write(runtime / "resend-api-key", resend.encode(), uid=1000, mode=0o400)
    if enabled:
        atomic_write(runtime / "plaid-secret", plaid_secret.encode(), uid=1000, mode=0o400)
    else:
        (runtime / "plaid-secret").unlink(missing_ok=True)
    atomic_write(runtime / "secrets.compose.json", json.dumps(override).encode())
    atomic_write(runtime / "compose.env", "".join(key + "=" + value + "\n" for key, value in environment.items()).encode())
    atomic_write(docker / "config.json", json.dumps({"auths": {config["registry"]: {"auth": auth}}}).encode())


def main():
    try:
        config = json.loads(CONFIG.read_text())
        project = metadata("project/project-id")
        token = metadata("instance/service-accounts/default/token", json_value=True)["access_token"]
        load(config, project, token)
    except urllib.error.HTTPError as error:
        print(f"Credential initialization failed: HTTP {error.code}", file=sys.stderr)
        return 1
    except (OSError, ValueError, KeyError, TypeError):
        print("Credential initialization failed; check configuration, secret versions, and IAM", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
