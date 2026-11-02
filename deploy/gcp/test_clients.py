"""CLI, credential export, fingerprint and disposable-image test contracts."""
import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import unittest
import urllib.error
from unittest.mock import MagicMock, patch

import test_deployment as deployment_tests
from test_deployment import HERE, inspection, module
from test_release import BASE, RUN
from test_runtime import response

client = module("release_client_tests", "release-client.py")
export = module("export_tests", "export-credentials.py")
gate = module("environment_gate_tests", "check-production-environment.py")
metadata = module("fingerprint_tests", "release-metadata.py")
smoke = module("smoke_tests", "smoke-image.py")


class ClientTests(unittest.TestCase):
    def argv(self, operation="deploy"):
        args = ["release-client.py", operation, "--project", "family-finance-trial", "--instance", "wealthfolio", "--zone", "us-west1-b"]
        if operation != "status": args += ["--image", BASE + "a" * 64, "--commit", "1" * 40, "--run-url", RUN]
        return args

    def test_client_sends_only_identifiers_and_status_omits_release_fields(self):
        state = {"current": {"image": BASE + "a" * 64}, "successful": ["not-for-ci"], "recovery_required": True}
        for operation, key in [("deploy", ""), ("rollback", "/private/ssh"), ("status", "")]:
            result = subprocess.CompletedProcess([], 0, json.dumps(state).encode(), b"")
            with self.subTest(operation=operation), patch.object(sys, "argv", self.argv(operation)), patch.dict(os.environ, {"WF_GCP_SSH_KEY_FILE": key}), patch.object(client.subprocess, "run", return_value=result) as execute, contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(client.main(), 0)
            args = execute.call_args.args[0]
            request = json.loads(execute.call_args.kwargs["input"])
            self.assertEqual(args[args.index("--command") + 1], "sudo /opt/wealthfolio/release.py")
            self.assertIn("--tunnel-through-iap", args)
            self.assertEqual("--ssh-key-file" in args, bool(key))
            self.assertEqual(set(request), {"operation"} if operation == "status" else {"operation", "image", "commit", "run_url"})
            self.assertNotIn("successful", output.getvalue())
            self.assertTrue(json.loads(output.getvalue())["recovery_required"])

    def test_client_validation_precedes_network_and_does_not_accept_moving_tags(self):
        cases = []
        for name, value in [("--image", "repo:latest"), ("--commit", "main"), ("--run-url", "https://attacker/run")]:
            args = self.argv(); args[args.index(name) + 1] = value; cases.append(args)
            args = self.argv(); index = args.index(name); del args[index:index + 2]; cases.append(args)
        for argv in cases:
            with self.subTest(argv=argv), patch.object(sys, "argv", argv), patch.object(client.subprocess, "run") as execute, contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit): client.main()
            execute.assert_not_called()

    def test_client_ssh_failure_hides_captured_output(self):
        result = subprocess.CompletedProcess([], 1, b"private", b"private")
        with patch.object(sys, "argv", self.argv("status")), patch.object(client.subprocess, "run", return_value=result), contextlib.redirect_stderr(io.StringIO()) as output:
            self.assertEqual(client.main(), 1)
        self.assertNotIn("private", output.getvalue())

    def test_production_gate_fetches_branch_policy_and_fails_closed(self):
        reviewer = {"protection_rules": [{"type": "required_reviewers", "reviewers": [{"id": 1}]}]}
        for custom in [True, False]:
            environment = dict(reviewer, deployment_branch_policy={"custom_branch_policies": True} if custom else {"protected_branches": True})
            responses = [response(environment), response({"branch_policies": [{"name": "main", "type": "branch"}]})]
            with self.subTest(custom=custom), patch.dict(os.environ, {"GH_TOKEN": "synthetic"}), patch.object(gate.urllib.request, "urlopen", side_effect=responses) as get, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(gate.main(), 0)
            self.assertEqual(get.call_count, 2 if custom else 1)
            self.assertEqual(get.call_args.args[0].get_header("Authorization"), "Bearer synthetic")
        for result in [OSError("private"), response({})]:
            with patch.dict(os.environ, {"GH_TOKEN": "synthetic"}), patch.object(gate.urllib.request, "urlopen", side_effect=result if isinstance(result, OSError) else None, return_value=result), contextlib.redirect_stderr(io.StringIO()) as output:
                self.assertEqual(gate.main(), 1)
                self.assertNotIn("private", output.getvalue())
        with self.assertRaises(ValueError): gate.validate(reviewer, {})


class ExportFingerprintTests(unittest.TestCase):
    setUp = deployment_tests.DeploymentTests.setUp

    def record(self, environment):
        return [{"Config": {"Image": "wealthfolio:test", "Env": environment}}]

    def test_export_reads_selected_inline_or_file_keys_without_exposing_other_variables(self):
        for files in [False, True]:
            environment = ["UNRELATED=private-other", "NO_ASSIGNMENT", "WF_SIGNUP_INVITE_HASH=" + "a" * 64]
            environment += ["WF_SECRET_KEY_FILE=/keys/master", "WF_RESEND_API_KEY_FILE=/keys/resend"] if files else ["WF_SECRET_KEY= master ", "WF_RESEND_API_KEY= resend "]
            values = [json.dumps(self.record(environment)).encode(), b"master\n", b"resend\n"]
            with self.subTest(files=files), patch.object(export.subprocess, "check_output", side_effect=values) as execute:
                self.assertEqual(export.export("wealthfolio"), {"WF_SECRET_KEY": "master", "WF_RESEND_API_KEY": "resend", "WF_SIGNUP_INVITE_HASH": "a" * 64})
                if files:
                    self.assertTrue(all(call.args[0][:4] == ["docker", "exec", "--user", "0"] for call in execute.call_args_list[1:]))

    def test_export_rejects_wrong_container_ambiguous_inputs_and_missing_master(self):
        with patch.object(export.subprocess, "check_output") as execute, self.assertRaises(ValueError): export.export("--unsafe")
        execute.assert_not_called()
        for records in [[{"Config": {"Image": "other:test"}}], self.record(["WF_SECRET_KEY=inline", "WF_SECRET_KEY_FILE=/keys/master"]), self.record([])]:
            with self.subTest(records=records), patch.object(export.subprocess, "check_output", return_value=json.dumps(records).encode()), self.assertRaises(ValueError): export.export("wealthfolio")

    def test_inventory_handles_empty_host_and_preserves_public_mount_information(self):
        with patch.object(inspection.subprocess, "check_output", return_value="") as execute:
            self.assertEqual(inspection.inventory(), {"containers": []})
            self.assertEqual(execute.call_count, 1)
        record = {"Name": "/wealthfolio", "Config": {"Image": "wealthfolio:test", "Labels": {"com.docker.compose.project": "wealthfolio"}, "Env": ["UNASSIGNED", "WF_SECRET_KEY_FILE=/keys/master", "WF_SIGNUP_INVITE_HASH=private"]},
                  "State": {"Running": False}, "HostConfig": {}, "Mounts": [{"Type": "volume", "Name": "existing", "Destination": "/data", "RW": True}], "NetworkSettings": {"Networks": {"app": {}}}}
        with patch.object(inspection.subprocess, "check_output", side_effect=["id", json.dumps([record]).encode()]): result = inspection.inventory()["containers"][0]
        self.assertEqual(result["mounts"][0]["Name"], "existing")
        self.assertEqual(result["networks"], ["app"])
        self.assertEqual(result["credential_inputs_present"], ["WF_SECRET_KEY_FILE", "WF_SIGNUP_INVITE_HASH"])
        self.assertNotIn("private", json.dumps(result))

    def test_fingerprint_changes_with_content_or_path_but_not_listing_order(self):
        (self.root / "a").write_text("backend"); (self.root / "b").write_text("config")
        with patch.object(metadata.subprocess, "check_output", return_value=b"a\0b\0"): first = metadata.fingerprint(self.root)
        with patch.object(metadata.subprocess, "check_output", return_value=b"b\0a\0"): self.assertEqual(metadata.fingerprint(self.root), first)
        (self.root / "b").write_text("changed")
        with patch.object(metadata.subprocess, "check_output", return_value=b"a\0b\0"): self.assertNotEqual(metadata.fingerprint(self.root), first)
        (self.root / "c").write_text("config")
        with patch.object(metadata.subprocess, "check_output", return_value=b"a\0c\0"): self.assertNotEqual(metadata.fingerprint(self.root), first)

    def test_safe_script_entrypoints_and_export_terminal_refusal(self):
        # Execute the script entrypoints while replacing only external boundaries.
        for tty, fail, expected in [(True, False, 1), (False, True, 1), (False, False, 0)]:
            result = json.dumps(self.record(["WF_SECRET_KEY=synthetic"])).encode()
            with self.subTest(tty=tty, fail=fail), patch.object(sys, "argv", ["export-credentials.py", "wealthfolio"]), patch.object(sys.stdout, "isatty", return_value=tty), patch.object(subprocess, "check_output", side_effect=OSError("private") if fail else None, return_value=result) as execute, contextlib.redirect_stderr(io.StringIO()) as error:
                # Preserve a controllable isatty on the redirected private pipe.
                output = io.StringIO(); output.isatty = lambda: tty
                with contextlib.redirect_stdout(output):
                    if expected:
                        with self.assertRaises(SystemExit) as caught: runpy.run_path(str(HERE / "export-credentials.py"), run_name="__main__")
                        self.assertEqual(caught.exception.code, 1)
                    else:
                        runpy.run_path(str(HERE / "export-credentials.py"), run_name="__main__")
                        self.assertEqual(json.loads(output.getvalue()), {"WF_SECRET_KEY": "synthetic"})
                self.assertNotIn("private", error.getvalue())
                if tty: execute.assert_not_called()
        with patch.object(subprocess, "check_output", return_value=""), contextlib.redirect_stdout(io.StringIO()) as output:
            runpy.run_path(str(HERE / "inspect-vm.py"), run_name="__main__")
            self.assertEqual(json.loads(output.getvalue()), {"containers": []})
        with patch.object(subprocess, "check_output", return_value=b""), contextlib.redirect_stdout(io.StringIO()) as output:
            runpy.run_path(str(HERE / "release-metadata.py"), run_name="__main__")
            self.assertEqual(len(output.getvalue().strip()), 64)


class SmokeTests(unittest.TestCase):
    def test_docker_wrapper_returns_output_and_redacts_failures(self):
        for code, check in [(0, True), (1, False), (1, True)]:
            with self.subTest(code=code, check=check), patch.object(smoke.subprocess, "run", return_value=subprocess.CompletedProcess([], code, " private ", "private")):
                if code and check:
                    with self.assertRaises(RuntimeError) as caught: smoke.docker("run", check=check)
                    self.assertNotIn("private", str(caught.exception))
                else: self.assertEqual(smoke.docker("run", check=check), "private")

    def run_smoke(self, mode):
        calls, requests = [], []
        health_calls = 0
        signup_calls = 0
        def docker(*args, **kwargs):
            calls.append(args)
            if args[0] == "port": return "127.0.0.1:12345"
            if mode == "docker-failure" and args[:2] == ("run", "-d"): raise RuntimeError("synthetic Docker failure")
            return ""
        def urlopen(request, **kwargs):
            nonlocal health_calls, signup_calls
            requests.append(request)
            path = request.full_url.split("/api/v1")[1]
            if path == "/healthz":
                health_calls += 1
                if mode == "retry" and health_calls == 1: raise OSError("not-ready")
                if mode == "bad-health" and health_calls == 1: return response({}, 503)
                return response({})
            if path == "/auth/status": return response({"accountAuth": mode != "disabled", "requiresPassword": True})
            if path == "/auth/me":
                if mode == "anonymous": return response({})
                raise urllib.error.HTTPError(request.full_url, 403 if mode == "wrong-denial" else 401, "synthetic", {}, None)
            if path == "/auth/signup":
                signup_calls += 1
                if signup_calls == 1:
                    body = json.loads(request.data)
                    self.assertEqual(body["invitationCode"], "synthetic-ci-invitation")
                    return response({}, 201)
                if mode == "reopened": return response({}, 201)
                raise urllib.error.HTTPError(request.full_url, 500 if mode == "wrong-signup" else 400, "synthetic", {}, None)
            if path == "/auth/login": return response({})
            self.fail("Unexpected outbound request: " + path)
        with patch.object(smoke, "docker", side_effect=docker), patch.object(smoke.urllib.request, "urlopen", side_effect=urlopen), patch.object(smoke.time, "sleep"), contextlib.redirect_stdout(io.StringIO()):
            try:
                smoke.check_image("synthetic:test")
            finally:
                self.assertEqual(calls[-2][0:3], ("rm", "-f", "-v"))
                self.assertEqual(calls[-1][0:2], ("volume", "rm"))
                self.assertFalse(any("recovery" in request.full_url for request in requests))
        self.assertTrue(any(args[0] == "restart" for args in calls))
        return calls

    def test_smoke_checks_signup_login_restart_and_cleans_disposable_resources(self):
        for mode in ["success", "retry", "bad-health"]:
            with self.subTest(mode=mode):
                calls = self.run_smoke(mode)
                start = next(args for args in calls if args[:2] == ("run", "-d"))
                self.assertIn("WF_RESEND_API_KEY_FILE=/run/secrets/resend", start)
                self.assertFalse(any(arg.startswith("WF_RESEND_API_KEY=") for arg in start))

    def test_smoke_fails_and_cleans_up_for_auth_and_container_failures(self):
        for mode in ["disabled", "anonymous", "wrong-denial", "reopened", "wrong-signup", "docker-failure"]:
            with self.subTest(mode=mode), self.assertRaises((AssertionError, RuntimeError)):
                self.run_smoke(mode)

    def test_smoke_health_timeout_cleans_up(self):
        calls = []
        def docker(*args, **kwargs):
            calls.append(args)
            return "127.0.0.1:12345" if args[0] == "port" else ""
        with patch.object(smoke, "docker", side_effect=docker), patch.object(smoke.time, "monotonic", side_effect=[0, 61]), self.assertRaisesRegex(RuntimeError, "failed startup"):
            smoke.check_image("synthetic:test")
        self.assertEqual(calls[-1][:2], ("volume", "rm"))
