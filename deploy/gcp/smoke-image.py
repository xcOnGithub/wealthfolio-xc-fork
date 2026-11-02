#!/usr/bin/env python3
"""Test owner-auth/key-file behavior in a disposable image; never request email."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid


def docker(*arguments, check=True):
    result = subprocess.run(["docker", *arguments], capture_output=True, text=True, timeout=120)
    if check and result.returncode:
        raise RuntimeError("Image smoke Docker operation failed")
    return result.stdout.strip()


def check_image(image):
    name = "wealthfolio-owner-smoke-" + uuid.uuid4().hex[:12]
    volume = name + "-data"
    invitation = "synthetic-ci-invitation"
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        (root / "master").write_bytes(base64.b64encode(b"c" * 32))
        (root / "resend").write_text("test-only-not-a-real-resend-key")
        docker("volume", "create", volume)
        try:
            # The runtime is UID 1000; adjust only these disposable test files.
            docker("run", "--rm", "--network=none", "--user=0", "--entrypoint=sh", "-v", str(root) + ":/keys", image,
                   "-c", "chown 1000:1000 /keys/master /keys/resend && chmod 400 /keys/master /keys/resend && test -s /etc/ssl/cert.pem")
            docker("run", "-d", "--name", name, "-v", volume + ":/data",
                   "--mount", "type=bind,src=" + str(root / "master") + ",dst=/run/secrets/master,readonly",
                   "--mount", "type=bind,src=" + str(root / "resend") + ",dst=/run/secrets/resend,readonly",
                   "-p", "127.0.0.1::8088", "-e", "WF_SECRET_KEY_FILE=/run/secrets/master",
                   "-e", "WF_RESEND_API_KEY_FILE=/run/secrets/resend", "-e", "WF_ACCOUNT_AUTH=true",
                   "-e", "WF_SIGNUP_INVITE_HASH=" + hashlib.sha256(invitation.encode()).hexdigest(),
                   "-e", "WF_AUTH_EMAIL_FROM=test@example.invalid", "-e", "WF_CORS_ALLOW_ORIGINS=https://ci.example.invalid",
                   "-e", "WF_COOKIE_SECURE=true", "-e", "WF_DB_REQUIRE_ENCRYPTION=true", image)
            origin = "http://" + docker("port", name, "8088/tcp")

            def request(path, body=None):
                data = json.dumps(body).encode() if body is not None else None
                req = urllib.request.Request(origin + "/api/v1" + path, data=data, headers={"Content-Type": "application/json"})
                with urllib.request.urlopen(req, timeout=5) as response:
                    return response.status, response.read()

            def healthy():
                deadline = time.monotonic() + 60
                while time.monotonic() < deadline:
                    try:
                        if request("/healthz")[0] == 200:
                            return
                    except OSError:
                        time.sleep(0.5)
                raise RuntimeError("Owner-auth image failed startup")

            healthy()
            status = json.loads(request("/auth/status")[1])
            assert status["accountAuth"] is True and status["requiresPassword"] is True
            try:
                request("/auth/me")
            except urllib.error.HTTPError as error:
                assert error.code == 401
            else:
                raise AssertionError("Anonymous access was admitted")
            owner = {"email": "owner@example.invalid", "password": "synthetic image test password"}
            assert request("/auth/signup", dict(owner, invitationCode=invitation))[0] == 201
            assert request("/auth/login", owner)[0] == 200
            docker("restart", name)
            healthy()
            assert request("/auth/login", owner)[0] == 200
            try:
                request("/auth/signup", dict(owner, invitationCode=invitation))
            except urllib.error.HTTPError as error:
                assert error.code == 400
            else:
                raise AssertionError("Signup reopened after restart")
            print("PASS owner signup/login/restart persistence, private auth, key-file inputs, and CA bundle")
        finally:
            docker("rm", "-f", "-v", name, check=False)
            docker("volume", "rm", volume, check=False)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True)
    check_image(parser.parse_args().image)
