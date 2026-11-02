"""Host release and credential boundary tests; no live secrets or host operations."""
import base64
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tarfile
import unittest
import urllib.error
from types import SimpleNamespace
from unittest.mock import MagicMock, mock_open, patch

import test_deployment as deployment_tests
from test_deployment import runtime
import test_release as release_tests
from test_release import BASE, RUN, release


def response(value, status=200):
    body = io.BytesIO(json.dumps(value).encode())
    body.status = status
    result = MagicMock()
    result.__enter__.return_value = body
    return result


class RuntimeTests(unittest.TestCase):
    setUp = deployment_tests.DeploymentTests.setUp
    load = deployment_tests.DeploymentTests.load

    def test_http_clients_send_metadata_flavor_and_secret_auth_to_correct_hosts(self):
        with patch.object(runtime.urllib.request, "urlopen", return_value=response({"answer": 1})) as get:
            self.assertEqual(runtime.http_json("https://secretmanager.googleapis.com/v1/key:access", {"Authorization": "Bearer synthetic"}), {"answer": 1})
            self.assertEqual(get.call_args.args[0].get_header("Authorization"), "Bearer synthetic")
            self.assertEqual(get.call_args.kwargs["timeout"], 20)
        for json_value, payload, expected in [(True, {"access_token": "synthetic"}, {"access_token": "synthetic"}), (False, "family-finance-trial", '"family-finance-trial"')]:
            with self.subTest(json_value=json_value), patch.object(runtime.urllib.request, "urlopen", return_value=response(payload)) as get:
                self.assertEqual(runtime.metadata("project/project-id", json_value=json_value), expected)
                request = get.call_args.args[0]
                self.assertEqual(request.get_header("Metadata-flavor"), "Google")
                self.assertNotIn("Authorization", request.headers)
                self.assertTrue(request.full_url.startswith(runtime.METADATA))

    def test_atomic_write_failure_preserves_old_file_and_cleans_temporary(self):
        target = self.root / "secret"
        target.write_bytes(b"old")
        with patch.object(runtime.os, "fchown"), patch.object(runtime.os, "replace", side_effect=OSError("disk error")):
            with self.assertRaises(OSError): runtime.atomic_write(target, b"new")
        self.assertEqual(target.read_bytes(), b"old")
        self.assertEqual(list(self.root.glob(".secret-*")), [])

    def test_config_rejects_shared_vm_unsafe_storage_missing_secrets_and_noninteger_port(self):
        cases = [{"dedicated_vm": False}, {"data_dir": "relative"}, {"data_volume": "--option"}, {"bind_port": "8088"}, {"secrets": {}}, {"email_from": ""}]
        for change in cases:
            with self.subTest(change=change), self.assertRaises(ValueError):
                runtime.validate_config(dict(self.config, **change), self.config["project"])
        # External proxy mode must not enable or create a bundled proxy.
        self.load(config=dict(self.config, manage_proxy=False))
        self.assertIn("COMPOSE_PROFILES=\n", (self.root / "runtime/compose.env").read_text())

    def test_all_invalid_secret_payloads_leave_previous_key_untouched(self):
        directory = self.root / "runtime"; directory.mkdir()
        key = directory / "master-key"; key.write_bytes(b"original")
        cases = [("master_key", b"x" * 4097), ("master_key", b"\xff"), ("resend_api_key", b""), ("resend_api_key", b"x" * 4097), ("resend_api_key", b"a\x01b"), ("resend_api_key", b"\xff"), ("invitation_hash", b"\xff")]
        for name, value in cases:
            with self.subTest(name=name, value=value[:5]), self.assertRaises((ValueError, UnicodeError)):
                self.load(values=dict(self.values, **{name: value}))
            self.assertEqual(key.read_bytes(), b"original")

    def test_credential_main_reports_http_status_without_private_error_body(self):
        path = self.root / "config.json"; path.write_text(json.dumps(self.config))
        for error in [urllib.error.HTTPError("https://private", 403, "private body", {}, None), OSError("private body")]:
            with self.subTest(error=type(error)), patch.object(runtime, "CONFIG", path), patch.object(runtime, "metadata", side_effect=error), contextlib.redirect_stderr(io.StringIO()) as output:
                self.assertEqual(runtime.main(), 1)
                self.assertNotIn("private body", output.getvalue())
                if isinstance(error, urllib.error.HTTPError): self.assertIn("HTTP 403", output.getvalue())


class ReleaseBoundaryTests(unittest.TestCase):
    setUp = release_tests.ReleaseTests.setUp
    fake_run = release_tests.ReleaseTests.fake_run
    request = release_tests.ReleaseTests.request

    def test_command_never_echoes_subprocess_output_on_failure(self):
        for code in [0, 1]:
            with self.subTest(code=code), patch.object(release.subprocess, "run", return_value=subprocess.CompletedProcess([], code, b"private", b"private")):
                if code:
                    with self.assertRaises(RuntimeError) as caught: release.command(["docker", "inspect"])
                    self.assertNotIn("private", str(caught.exception))
                else: self.assertEqual(release.command(["docker", "inspect"]), "private")

    def test_release_request_rejects_unknown_operation_and_extra_status_fields(self):
        for request in [{"operation": "erase"}, {"operation": "status", "image": "extra"}, dict(self.request(), run_url="https://other/run")]:
            with self.subTest(request=request), self.assertRaises(ValueError): release.validate_request(request, self.config)

    def test_image_labels_require_valid_commit_fingerprint_and_contract(self):
        labels = self.records[BASE + "a" * 64]
        for change in [None, {"org.opencontainers.image.revision": "main"}, {"org.wealthfolio.state-fingerprint": "bad"}]:
            candidate = None if change is None else dict(labels, **change)
            with self.subTest(change=change), self.assertRaises(ValueError):
                release.image_metadata(BASE + "a" * 64, run=lambda _: json.dumps(candidate))

    def test_backup_is_private_readable_and_handles_nested_files_and_symlinks(self):
        (self.data / "nested").mkdir(); (self.data / "nested/file").write_text("financial data")
        (self.data / "alias").symlink_to("web-account.json")
        (self.data / "dangling").symlink_to("missing")
        backup = Path(release.backup_data(self.data, self.root / "backups"))
        self.assertEqual(stat.S_IMODE(backup.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(backup.parent.stat().st_mode), 0o700)
        with tarfile.open(backup) as archive:
            self.assertEqual(archive.extractfile("data/web-account.json").read(), b"original-owner")
            self.assertTrue(archive.getmember("data/alias").issym())
            self.assertIn("data/nested/file", archive.getnames())

    def test_unsafe_empty_missing_and_low_space_backups_fail_without_archive(self):
        empty = self.root / "empty"; empty.mkdir()
        for data, destination in [(self.data, self.data), (self.data, self.data / "nested"), (empty, self.root / "backups"), (self.root / "missing", self.root / "backups")]:
            with self.subTest(data=data, destination=destination), self.assertRaises(ValueError): release.backup_data(data, destination)
        with patch.object(shutil, "disk_usage", return_value=SimpleNamespace(free=0)):
            with self.assertRaisesRegex(ValueError, "Insufficient space"): release.backup_data(self.data, self.root / "backups")
        self.assertEqual(list((self.root / "backups").iterdir()), [])

    def test_failed_archive_is_deleted_and_live_data_is_never_changed(self):
        with patch.object(release.tarfile, "open", side_effect=OSError("compression failure")):
            with self.assertRaises(OSError): release.backup_data(self.data, self.root / "backups")
        self.assertEqual(list((self.root / "backups").iterdir()), [])
        self.assertEqual((self.data / "web-account.json").read_text(), "original-owner")

    def test_prepare_pull_uses_temporary_registry_identity_and_preserves_active_keys(self):
        active = self.root / "active"; active.mkdir(); (active / "master-key").write_bytes(b"original-key")
        pulls = []
        def load(config, project, token, directory):
            self.assertEqual((project, token), ("family-finance-trial", "synthetic"))
            (directory / "master-key").write_bytes(b"original-key")
        def run(arguments, env=None):
            if arguments[:2] == ["docker", "pull"]:
                pulls.append(Path(env["DOCKER_CONFIG"]).parent)
                self.assertTrue((pulls[-1] / "master-key").exists())
            return ""
        factory = release.tempfile.TemporaryDirectory
        def temporary(**kwargs): return factory(prefix=kwargs["prefix"], dir=self.root)
        with patch.object(release.runtime, "metadata", side_effect=["family-finance-trial", {"access_token": "synthetic"}]), patch.object(release.runtime, "load", side_effect=load), patch.object(release.runtime, "RUNTIME", active), patch.object(release.tempfile, "TemporaryDirectory", side_effect=temporary):
            self.manager.run = run
            self.manager.prepare_image(self.config, BASE + "b" * 64)
        self.assertEqual(len(pulls), 1)
        self.assertFalse(pulls[0].exists())
        self.assertEqual((active / "master-key").read_bytes(), b"original-key")

    def test_prepare_refuses_key_change_pull_failure_and_volume_drift_before_stop(self):
        active = self.root / "active"; active.mkdir(); (active / "master-key").write_bytes(b"original-key")
        factory = release.tempfile.TemporaryDirectory
        def temporary(**kwargs): return factory(prefix=kwargs["prefix"], dir=self.root)
        for mode in ["key-change", "pull-failure", "remote-driver", "options", "moved", "valid"]:
            def load(config, project, token, directory): (directory / "master-key").write_bytes(b"different" if mode == "key-change" else b"original-key")
            volume = {"Driver": "nfs" if mode == "remote-driver" else "local", "Options": {"device": "remote"} if mode == "options" else None, "Mountpoint": str(self.data if mode != "moved" else self.root / "different")}
            calls = []
            def run(arguments, **kwargs):
                calls.append(arguments)
                if arguments[:2] == ["docker", "pull"] and mode == "pull-failure": raise RuntimeError("pull failed")
                return json.dumps([volume])
            with self.subTest(mode=mode), patch.object(release.runtime, "metadata", side_effect=["family-finance-trial", {"access_token": "synthetic"}]), patch.object(release.runtime, "load", side_effect=load), patch.object(release.runtime, "RUNTIME", active), patch.object(release.tempfile, "TemporaryDirectory", side_effect=temporary):
                self.manager.run = run
                config = dict(self.config, data_volume="existing")
                if mode == "valid": self.manager.prepare_image(config, BASE + "b" * 64)
                else:
                    with self.assertRaises((ValueError, RuntimeError)): self.manager.prepare_image(config, BASE + "b" * 64)
            self.assertFalse(any(args[0] == "systemctl" for args in calls))
            if mode == "key-change": self.assertEqual(calls, [])

    def test_noop_release_checks_health_without_backing_up_or_stopping(self):
        health = MagicMock(); self.manager.health = health
        self.manager.apply(self.request(digest="a"))
        health.assert_called_once_with(self.config, BASE + "a" * 64)
        self.assertFalse(any(args[0] == "systemctl" for args in self.commands))

    def test_rollback_between_different_successful_backends_is_blocked(self):
        self.records[BASE + "b" * 64]["org.wealthfolio.state-fingerprint"] = "b" * 64
        self.manager.apply(self.request())
        self.commands.clear()
        with self.assertRaisesRegex(ValueError, "Backend-state changes"):
            self.manager.apply(self.request("rollback", "a"))
        self.assertFalse(any(args[0] == "systemctl" for args in self.commands))

    def test_initialization_cannot_replace_history_even_during_recovery(self):
        state = self.manager.state(); state["recovery_required"] = True; self.manager.save(state)
        self.config_path.write_text(json.dumps(dict(self.config, image=BASE + "b" * 64)))
        with self.assertRaisesRegex(ValueError, "cannot be replaced"): self.manager.initialize()
        self.assertEqual(self.manager.state()["current"]["image"], BASE + "a" * 64)

    def test_health_verification_rejects_bad_status_and_unexpected_private_response(self):
        config = {"bind_port": 8088, "domain": "app.example.com"}
        denied = urllib.error.HTTPError("url", 503, "not-ready", {}, None)
        for responses in [[response({}, 503)], [response({}), response({"accountAuth": True, "requiresPassword": True}), denied]]:
            with patch.object(release.urllib.request, "urlopen", side_effect=responses), self.assertRaises(ValueError):
                release.verify(config, BASE + "a" * 64, run=lambda _: "same-id")

    def test_default_health_adapter_calls_image_verification(self):
        manager = release.Releases(self.config_path, self.root / "other", run=self.fake_run)
        with patch.object(release, "verify") as verify:
            manager.health(self.config, self.config["image"])
            verify.assert_called_once_with(self.config, self.config["image"], self.fake_run)

    def test_release_main_limits_requests_and_holds_lock_before_dispatch(self):
        for arguments, source, action in [(["--initialize"], "", "initialize"), ([], '{"operation":"status"}', "apply"), ([], "x" * 8193, None), (["--invalid"], "", None), ([], "invalid-json", None)]:
            manager = MagicMock(); manager.initialize.return_value = {"current": "baseline"}; manager.apply.return_value = {"current": "baseline"}
            lock = mock_open()
            with self.subTest(arguments=arguments, length=len(source)), patch.object(sys, "argv", ["release.py", *arguments]), patch.object(sys, "stdin", io.StringIO(source)), patch.object(release, "Releases", return_value=manager), patch.object(release, "open", lock, create=True), patch.object(release.Path, "mkdir"), patch.object(release.os, "umask"), patch.object(release.fcntl, "flock") as flock, contextlib.redirect_stdout(io.StringIO()) as output, contextlib.redirect_stderr(io.StringIO()) as error:
                self.assertEqual(release.main(), 0 if action else 1)
                flock.assert_called_once()
                if action: self.assertEqual(json.loads(output.getvalue()), {"current": "baseline"})
                else:
                    manager.initialize.assert_not_called(); manager.apply.assert_not_called()
                    self.assertIn("No data restore", error.getvalue())
        for exception, message in [(OSError("private"), "Inspect the installed"), (RuntimeError("safe failure"), "safe failure")]:
            with patch.object(sys, "argv", ["release.py"]), patch.object(sys, "stdin", io.StringIO('{"operation":"status"}')), patch.object(release, "Releases", side_effect=exception), patch.object(release, "open", mock_open(), create=True), patch.object(release.Path, "mkdir"), patch.object(release.os, "umask"), patch.object(release.fcntl, "flock"), contextlib.redirect_stderr(io.StringIO()) as output:
                self.assertEqual(release.main(), 1)
                self.assertIn(message, output.getvalue()); self.assertNotIn("private", output.getvalue())
