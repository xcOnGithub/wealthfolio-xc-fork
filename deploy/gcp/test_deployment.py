import base64
import argparse
import contextlib
import copy
import importlib.util
import io
import json
import re
import shlex
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).parent


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


runtime = module("runtime", "fetch-secrets.py")
manage = module("manage", "manage.py")
inspection = module("inspection", "inspect-vm.py")


class DeploymentTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        data = self.root / "data"
        data.mkdir()
        self.config = {
            "project": "example-finance-project", "domain": "portfolio.example.com",
            "registry": "us-west1-docker.pkg.dev",
            "image": "us-west1-docker.pkg.dev/example-finance-project/wealthfolio/server@sha256:" + "a" * 64,
            "proxy_image": "caddy@sha256:" + "b" * 64,
            "email_from": "Portfolio <sender@example.com>",
            "require_db_encryption": False, "manage_proxy": True, "dedicated_vm": True,
            "data_dir": str(data), "db_filename": "app.db", "bind_port": 8088,
            "previous_container": "wealthfolio-old",
            "secrets": {key: "projects/example-finance-project/secrets/" + value + "/versions/2" for key, value in manage.SECRET_IDS.items()},
        }
        self.values = {
            "master_key": base64.b64encode(b"x" * 32),
            "resend_api_key": b"re_test$literal-key",
            "invitation_hash": b"a" * 64,
        }

    def load(self, config=None, values=None):
        selected = values if values is not None else self.values
        mapping = {self.config["secrets"][key]: value for key, value in selected.items()}
        with patch.object(runtime, "read_secret", side_effect=lambda name, token: mapping[name]), patch.object(runtime.os, "fchown") as ownership:
            runtime.load(config or self.config, "example-finance-project", "test-token", self.root / "runtime")
        return ownership

    def test_keys_use_private_files_and_never_container_environment(self):
        ownership = self.load()
        directory = self.root / "runtime"
        for filename, expected in [("master-key", self.values["master_key"]), ("resend-api-key", self.values["resend_api_key"])]:
            path = directory / filename
            self.assertEqual(path.read_bytes(), expected)
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o400)
        for filename in ["compose.env", "secrets.compose.json", "docker/config.json"]:
            self.assertEqual(stat.S_IMODE((directory / filename).stat().st_mode), 0o600)
        override = json.loads((directory / "secrets.compose.json").read_text())
        environment = override["services"]["wealthfolio"]["environment"]
        self.assertNotIn("WF_SECRET_KEY", environment)
        self.assertNotIn("WF_RESEND_API_KEY", environment)
        self.assertNotIn(self.values["resend_api_key"].decode(), json.dumps(override))
        self.assertEqual(environment["WF_AUTH_EMAIL_FROM"], "Portfolio <sender@example.com>")
        self.assertEqual([call.args[1:] for call in ownership.call_args_list[:2]], [(1000, 1000), (1000, 1000)])
        auth = json.loads((directory / "docker/config.json").read_text())["auths"][self.config["registry"]]["auth"]
        self.assertEqual(base64.b64decode(auth), b"oauth2accesstoken:test-token")

    def test_invalid_payload_does_not_replace_existing_credentials(self):
        directory = self.root / "runtime"
        directory.mkdir()
        original = directory / "master-key"
        original.write_bytes(b"original")
        for key, invalid in [("invitation_hash", b"invalid"), ("resend_api_key", b"key\ninjection"), ("master_key", b"")]:
            with self.subTest(key=key):
                values = dict(self.values, **{key: invalid})
                with self.assertRaises(ValueError):
                    self.load(values=values)
                self.assertEqual(original.read_bytes(), b"original")
                self.assertFalse((directory / "secrets.compose.json").exists())

    def test_crc32c_known_vector_and_corruption_rejection(self):
        self.assertEqual(runtime.crc32c(b"123456789"), 0xE3069283)
        data = b"test-payload"
        payload = {"payload": {"data": base64.b64encode(data).decode(), "dataCrc32c": str(runtime.crc32c(data))}}
        with patch.object(runtime, "http_json", return_value=payload):
            self.assertEqual(runtime.read_secret(self.config["secrets"]["master_key"], "test-token"), data)
        payload["payload"]["dataCrc32c"] = "0"
        with patch.object(runtime, "http_json", return_value=payload):
            with self.assertRaisesRegex(ValueError, "checksum"):
                runtime.read_secret(self.config["secrets"]["master_key"], "test-token")

    def test_configuration_rejects_wrong_projects_unpinned_versions_and_injection(self):
        cases = [("project", "foreign-project"), ("domain", "example.com\nmalicious"),
                 ("registry", "attacker.example.com"), ("image", "server:latest"),
                 ("proxy_image", "caddy:latest"), ("data_dir", "/var/lib/docker"),
                 ("data_dir", "/srv/../etc"), ("db_filename", "../other.db"),
                 ("email_from", "sender@example.com\nHeader: injected"), ("bind_port", 80),
                 ("manage_proxy", "false"), ("require_db_encryption", "false"),
                 ("previous_container", "--unsafe")]
        for key, value in cases:
            with self.subTest(key=key, value=value):
                config = dict(self.config, **{key: value})
                with self.assertRaises(ValueError):
                    runtime.validate_config(config, "example-finance-project")
        config = copy.deepcopy(self.config)
        config["secrets"]["master_key"] = config["secrets"]["master_key"].rsplit("/", 1)[0] + "/latest"
        with self.assertRaises(ValueError):
            runtime.validate_config(config, "example-finance-project")

    def test_missing_data_fails_before_fetching_or_writing_secrets(self):
        config = dict(self.config, data_dir=str(self.root / "missing"))
        with patch.object(runtime, "read_secret") as reader:
            with self.assertRaisesRegex(ValueError, "persistent data"):
                runtime.load(config, "example-finance-project", "test-token", self.root / "runtime")
            reader.assert_not_called()
        self.assertFalse((self.root / "runtime").exists())

    def test_runtime_uses_installed_plan_instead_of_staged_vm_metadata(self):
        path = self.root / "installed.json"
        path.write_text(json.dumps(self.config))
        def metadata(name, **options):
            if name == "project/project-id":
                return "example-finance-project"
            if name == "instance/service-accounts/default/token":
                return {"access_token": "test-token"}
            self.fail("Staged metadata must not activate itself")
        with patch.object(runtime, "CONFIG", path), patch.object(runtime, "metadata", side_effect=metadata), patch.object(runtime, "load") as loader:
            self.assertEqual(runtime.main(), 0)
        self.assertEqual(loader.call_args.args[0], self.config)

    def test_dotenv_import_does_not_execute_or_interpolate_values(self):
        marker = self.root / "must-not-exist"
        path = self.root / "credentials.env"
        path.write_text("WF_RESEND_API_KEY='$(touch " + str(marker) + ")'\nUNRELATED_SECRET=not-imported\nWF_SIGNUP_INVITE_HASH=" + "a" * 64 + "\n")
        values = manage.read_env(path)
        self.assertEqual(values["WF_RESEND_API_KEY"], "$(touch " + str(marker) + ")")
        self.assertNotIn("UNRELATED_SECRET", values)
        self.assertFalse(marker.exists())

    def test_sdk_failures_do_not_log_captured_secret_payloads(self):
        failure = subprocess.CompletedProcess(["gcloud"], 1, b"private-key-value", b"private-key-value")
        with patch.object(manage.subprocess, "run", return_value=failure):
            with self.assertRaises(manage.DeploymentError) as caught:
                manage.gcloud(["secrets", "versions", "add", "test", "--data-file=-"], "example-finance-project", data=b"private-key-value")
        self.assertNotIn("private-key-value", str(caught.exception))

    def test_seed_uses_vm_credentials_over_local_test_keys_and_only_stdin(self):
        path = self.root / "local.env"
        path.write_text("WF_SECRET_KEY=local-test-key\nWF_RESEND_API_KEY=re_local-test\nWF_SIGNUP_INVITE_HASH=" + "c" * 64)
        actual = {"WF_SECRET_KEY": self.values["master_key"].decode(), "WF_RESEND_API_KEY": "re_existing-vm", "WF_SIGNUP_INVITE_HASH": "b" * 64}
        calls = []
        def cloud(arguments, project, **options):
            calls.append((arguments, options.get("data")))
            if arguments[:3] == ["secrets", "versions", "add"]:
                return json.dumps({"name": "projects/123456789/secrets/" + arguments[3] + "/versions/1"}).encode()
            return b"{}"
        args = argparse.Namespace(project="example-finance-project", env_file=path, instance="existing-vm", container="wealthfolio", zone="us-west1-b")
        output = io.StringIO()
        with patch.object(manage, "gcloud", side_effect=cloud), patch.object(manage, "enabled_version", return_value=None), patch.object(manage, "remote_script", return_value=json.dumps(actual).encode()), contextlib.redirect_stdout(output):
            manage.seed(args)
        uploads = [(arguments, payload) for arguments, payload in calls if payload]
        self.assertEqual([payload for _, payload in uploads], [value.encode() for value in actual.values()])
        for arguments, payload in uploads:
            self.assertNotIn(payload.decode(), " ".join(arguments))
            self.assertNotIn(payload.decode(), output.getvalue())
        self.assertNotIn("local-test-key", output.getvalue())
        self.assertTrue(all(name.startswith("projects/example-finance-project/") for name in json.loads(output.getvalue()).values()))

    def test_seed_refuses_to_rotate_an_existing_master_key(self):
        path = self.root / "local.env"
        path.write_text("WF_SECRET_KEY=new-key\nWF_RESEND_API_KEY=re_test\nWF_SIGNUP_INVITE_HASH=" + "a" * 64)
        args = argparse.Namespace(project="example-finance-project", env_file=path, instance=None)
        def cloud(arguments, project, **options):
            return json.dumps({"payload": {"data": base64.b64encode(b"original-key").decode()}}).encode() if arguments[:3] == ["secrets", "versions", "access"] else b"{}"
        with patch.object(manage, "gcloud", side_effect=cloud) as calls, patch.object(manage, "enabled_version", return_value="projects/example-finance-project/secrets/wealthfolio-master-key/versions/1"):
            with self.assertRaisesRegex(manage.DeploymentError, "Existing secret differs"):
                manage.seed(args)
        self.assertFalse(any(call.args[0][:3] == ["secrets", "versions", "add"] for call in calls.call_args_list))

    def test_seed_reuses_matching_json_encoded_secrets_without_adding_versions(self):
        path = self.root / "local.env"
        sources = dict(zip(["WF_SECRET_KEY", "WF_RESEND_API_KEY", "WF_SIGNUP_INVITE_HASH"], self.values.values()))
        path.write_text("\n".join(key + "=" + value.decode() for key, value in sources.items()))
        args = argparse.Namespace(project="example-finance-project", env_file=path, instance=None)
        values = {manage.SECRET_IDS[key]: value for key, value in self.values.items()}
        versions = {key: "projects/example-finance-project/secrets/" + secret + "/versions/2" for key, secret in manage.SECRET_IDS.items()}
        def cloud(arguments, project, **options):
            if arguments[:3] == ["secrets", "versions", "access"]:
                value = values[arguments[arguments.index("--secret") + 1]]
                return json.dumps({"payload": {"data": base64.b64encode(value).decode()}}).encode()
            return b"{}"
        output = io.StringIO()
        with patch.object(manage, "gcloud", side_effect=cloud) as calls, patch.object(manage, "enabled_version", side_effect=lambda project, secret: "projects/" + project + "/secrets/" + secret + "/versions/2"), contextlib.redirect_stdout(output):
            manage.seed(args)
        self.assertEqual(json.loads(output.getvalue()), versions)
        self.assertEqual(sum(call.args[0][:3] == ["secrets", "versions", "access"] for call in calls.call_args_list), 3)
        self.assertFalse(any(call.args[0][:3] == ["secrets", "versions", "add"] for call in calls.call_args_list))
        for value in self.values.values():
            self.assertNotIn(value.decode(), output.getvalue())

    def test_seed_rejects_malformed_access_responses_without_leaking_or_rotating(self):
        path = self.root / "local.env"
        path.write_text("WF_SECRET_KEY=original-key\nWF_RESEND_API_KEY=re_test\nWF_SIGNUP_INVITE_HASH=" + "a" * 64)
        args = argparse.Namespace(project="example-finance-project", env_file=path, instance=None)
        for response in [b"not-json-private-value", b'{}', b'{"payload":{"data":"not-base64-private-value!"}}']:
            def cloud(arguments, project, **options):
                return response if arguments[:3] == ["secrets", "versions", "access"] else b"{}"
            with self.subTest(response=response), patch.object(manage, "gcloud", side_effect=cloud) as calls, patch.object(manage, "enabled_version", return_value="projects/example-finance-project/secrets/wealthfolio-master-key/versions/1"):
                with self.assertRaisesRegex(manage.DeploymentError, "Invalid encoded Secret Manager response") as caught:
                    manage.seed(args)
            self.assertNotIn("private-value", str(caught.exception))
            self.assertFalse(any(call.args[0][:3] == ["secrets", "versions", "add"] for call in calls.call_args_list))

    def test_bootstrap_stops_service_when_initialization_fails(self):
        # Execute the actual shell control flow with mocked system commands;
        # never run the installer preflight or mutate host services/data.
        tail = (HERE / "bootstrap.sh").read_text().split("systemctl daemon-reload\n", 1)[1]
        tail = tail.replace("/usr/bin/python3 /opt/wealthfolio/release.py", "initialize")
        for status in [0, 1]:
            with self.subTest(status=status):
                log = self.root / "service.log"
                functions = "\n".join([
                    'systemctl() { echo "$*" >> ' + shlex.quote(str(log)) + '; }',
                    "docker() { echo healthy; }",
                    "flock() { :; }",
                    "initialize() { return " + str(status) + "; }",
                ])
                log.unlink(missing_ok=True)
                result = subprocess.run(["bash", "-c", "set -eu\n" + functions + "\n" + tail], capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, status)
                self.assertEqual("stop wealthfolio.service" in log.read_text(), status != 0)
                self.assertEqual("WEALTHFOLIO_BOOTSTRAP_READY" in result.stdout, status == 0)

    def test_inventory_exposes_only_allowlisted_environment_values(self):
        record = {"Name": "/wealthfolio", "Config": {"Image": "wealthfolio:test", "Labels": {}, "Env": ["WF_SECRET_KEY=private-key-value", "WF_RESEND_API_KEY=private-api-key", "WF_DB_PATH=/data/app.db"]},
                  "State": {"Running": True}, "HostConfig": {"PortBindings": {}}, "Mounts": []}
        with patch.object(inspection.subprocess, "check_output", side_effect=["container-id", json.dumps([record]).encode()]):
            result = inspection.inventory()
        rendered = json.dumps(result)
        self.assertNotIn("private-key-value", rendered)
        self.assertNotIn("private-api-key", rendered)
        self.assertEqual(result["containers"][0]["settings"], {"WF_DB_PATH": "/data/app.db"})

    def test_embedded_bootstrap_python_compiles_before_remote_use(self):
        blocks = re.findall(r"<<'PY'\n(.*?)\nPY\n", (HERE / "bootstrap.sh").read_text(), re.DOTALL)
        self.assertEqual(len(blocks), 2)
        for index, source in enumerate(blocks):
            compile(source, f"bootstrap-python-{index}", "exec")

    @unittest.skipUnless(shutil.which("docker"), "Docker Compose is required for the rendered configuration check")
    def test_compose_keeps_app_private_and_reads_keys_from_files(self):
        self.load()
        result = subprocess.run(["docker", "compose", "--env-file", str(self.root / "runtime/compose.env"),
                                 "-f", str(HERE / "compose.yaml"), "-f", str(self.root / "runtime/secrets.compose.json"),
                                 "config", "--format=json"], check=True, capture_output=True)
        config = json.loads(result.stdout)
        app = config["services"]["wealthfolio"]
        self.assertEqual(app["environment"]["WF_SECRET_KEY_FILE"], "/run/secrets/master-key")
        self.assertEqual(app["environment"]["WF_RESEND_API_KEY_FILE"], "/run/secrets/resend-api-key")
        self.assertNotIn("WF_RESEND_API_KEY", app["environment"])
        self.assertNotIn("WF_SECRET_KEY", app["environment"])
        self.assertEqual(app["ports"][0]["host_ip"], "127.0.0.1")
        self.assertEqual(app["restart"], "no")
        self.assertTrue(app["read_only"])
        self.assertEqual(app["environment"]["WF_DB_PATH"], "/data/app.db")
        self.assertEqual(app["environment"]["WF_AUTH_EMAIL_FROM"], self.config["email_from"])

    @unittest.skipUnless(shutil.which("docker"), "Docker Compose is required for the named-volume check")
    def test_named_volume_is_reused_as_external_without_creating_new_storage(self):
        self.load(config=dict(self.config, data_volume="existing-portfolio"))
        result = subprocess.run(["docker", "compose", "--env-file", str(self.root / "runtime/compose.env"),
                                 "-f", str(HERE / "compose.yaml"), "-f", str(self.root / "runtime/secrets.compose.json"),
                                 "config", "--format=json"], check=True, capture_output=True)
        config = json.loads(result.stdout)
        mounts = [entry for entry in config["services"]["wealthfolio"]["volumes"] if entry["target"] == "/data"]
        self.assertEqual(len(mounts), 1)
        self.assertEqual(mounts[0]["type"], "volume")
        self.assertEqual(mounts[0]["source"], "existing_data")
        self.assertEqual(config["volumes"]["existing_data"]["name"], "existing-portfolio")
        self.assertTrue(config["volumes"]["existing_data"]["external"])


if __name__ == "__main__":
    unittest.main()
