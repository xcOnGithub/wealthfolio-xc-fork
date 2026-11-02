"""Operator orchestration contracts with SDK/process boundaries replaced by fakes."""
import argparse
import base64
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

import test_deployment as deployment_tests
from test_deployment import HERE, manage, module
import test_release as release_tests

bootstrap = module("bootstrap_operator", "bootstrap-ci.py")


class OperatorTests(unittest.TestCase):
    setUp = deployment_tests.DeploymentTests.setUp

    def args(self, **changes):
        args = argparse.Namespace(project="family-finance-trial", instance="wealthfolio", zone="us-west1-b", region="us-west1")
        vars(args).update(changes)
        return args

    def test_sdk_distinguishes_missing_resources_from_permission_failures(self):
        for code, stderr, optional, expected in [(0, b"", False, b"{}"), (1, b"NOT_FOUND", True, None)]:
            with self.subTest(code=code), patch.object(manage.subprocess, "run", return_value=subprocess.CompletedProcess([], code, b"{}", stderr)):
                self.assertEqual(manage.run(["gcloud", "describe"], optional=optional), expected)
        with patch.object(manage.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, b"private", b"PERMISSION_DENIED private")):
            with self.assertRaises(manage.DeploymentError) as caught:
                manage.run(["gcloud", "describe"], optional=True)
        self.assertNotIn("private", str(caught.exception))

    def test_optional_resource_and_ssh_script_transport(self):
        with patch.object(manage, "gcloud", return_value=None):
            self.assertIsNone(manage.resource(["missing"], "family-finance-trial", optional=True))
        for key in ["", "/private/ssh-key"]:
            with self.subTest(key=key), patch.dict(os.environ, {"WF_GCP_SSH_KEY_FILE": key}), patch.object(manage, "gcloud", return_value=b"result") as cloud:
                result = manage.remote_script("family-finance-trial", "us-west1-b", "wealthfolio", "inspect-vm.py", ["literal;argument"])
                self.assertEqual(result, b"result")
                arguments = cloud.call_args.args[0]
                self.assertIn("--tunnel-through-iap", arguments)
                self.assertEqual("--ssh-key-file" in arguments, bool(key))
                self.assertIn("'literal;argument'", arguments[arguments.index("--command") + 1])

    def test_inventory_never_emits_legacy_metadata_values(self):
        vm = {"name": "wealthfolio", "metadata": {"items": [{"key": "legacy-key", "value": "private-value"}]}}
        output = io.StringIO()
        with patch.object(manage, "resource", return_value=vm), patch.object(manage, "remote_script", return_value=b'{"containers":[]}'), contextlib.redirect_stdout(output):
            manage.inspect(self.args())
        result = json.loads(output.getvalue())
        self.assertEqual(result["metadata_keys"], ["legacy-key"])
        self.assertEqual(result["docker"], {"containers": []})
        self.assertNotIn("private-value", output.getvalue())

    def test_dotenv_quotes_comments_exports_and_invalid_inputs(self):
        path = self.root / "input.env"
        path.write_text('\n# comment\nexport WF_SECRET_KEY="literal$key"\nWF_RESEND_API_KEY=\'test\'\nignored\nOTHER=x\n')
        self.assertEqual(manage.read_env(path), {"WF_SECRET_KEY": "literal$key", "WF_RESEND_API_KEY": "test"})
        for value in ["'unterminated", '"line\\nbreak"', '"nul\\u0000byte"']:
            with self.subTest(value=value):
                path.write_text("WF_SECRET_KEY=" + value)
                with self.assertRaises((manage.DeploymentError, ValueError)):
                    manage.read_env(path)

    def test_enabled_version_creates_only_missing_secrets_and_selects_latest_enabled(self):
        for responses, expected, creates in [([None], None, True), ([{}, []], None, False), ([{}, [{"name": "projects/123/secrets/key/versions/7"}]], "projects/family-finance-trial/secrets/key/versions/7", False)]:
            with self.subTest(responses=responses), patch.object(manage, "resource", side_effect=responses) as read, patch.object(manage, "gcloud") as write:
                self.assertEqual(manage.enabled_version("family-finance-trial", "key"), expected)
                self.assertEqual(write.called, creates)
                if len(responses) > 1:
                    self.assertIn("--filter=state:ENABLED", read.call_args.args[0])

    def test_seed_missing_inputs_fail_before_creating_versions(self):
        full = {"WF_SECRET_KEY": "original", "WF_RESEND_API_KEY": "test", "WF_SIGNUP_INVITE_HASH": "a" * 64}
        cases = [{}, {"WF_SECRET_KEY": "original"}, dict(full, WF_SIGNUP_INVITE_HASH=""), dict(full, WF_SIGNUP_INVITE_HASH="invalid")]
        for values in cases:
            with self.subTest(values=values), patch.object(manage, "read_env", return_value=values), patch.object(manage, "gcloud"), patch.object(manage, "enabled_version") as versions:
                with self.assertRaises(manage.DeploymentError):
                    manage.seed(self.args(instance=None, env_file=self.root / "env", container=None))
                versions.assert_not_called()
        with patch.object(manage, "gcloud"), patch.object(manage, "remote_script") as remote:
            with self.assertRaisesRegex(manage.DeploymentError, "Select the existing"):
                manage.seed(self.args(env_file=None, container=None))
            remote.assert_not_called()
        args = self.args(env_file=None, container="wealthfolio")
        for name in ["WF_SECRET_KEY", "WF_RESEND_API_KEY"]:
            values = dict(full)
            values.pop(name)
            with patch.object(manage, "remote_script", return_value=json.dumps(values)), patch.object(manage, "gcloud"), patch.object(manage, "enabled_version") as versions:
                with self.assertRaises(manage.DeploymentError):
                    manage.seed(args)
                versions.assert_not_called()

    def test_build_requires_clean_source_and_uses_committed_image_and_fingerprint(self):
        with patch.object(manage, "run", return_value=b" M source"), patch.object(manage, "gcloud") as cloud:
            with self.assertRaisesRegex(manage.DeploymentError, "Commit the exact source"):
                manage.build(self.args())
            cloud.assert_not_called()
        for exists in [False, True]:
            image = {"image_summary": {"digest": "sha256:" + "a" * 64}}
            resources = [{}, {}, {}, image] if exists else [None, None, None, None, image]
            output = io.StringIO()
            with self.subTest(exists=exists), patch.object(manage, "run", side_effect=[b"", b"1" * 40, b"2" * 64]), patch.object(manage, "resource", side_effect=resources), patch.object(manage, "gcloud") as cloud, contextlib.redirect_stdout(output):
                manage.build(self.args())
            calls = [call.args[0] for call in cloud.call_args_list]
            submits = [args for args in calls if args[:2] == ["builds", "submit"]]
            self.assertEqual(len(submits), int(not exists))
            if submits:
                self.assertIn("_STATE_FINGERPRINT=" + "2" * 64, submits[0][-1])
                self.assertIn("_COMMIT=" + "1" * 40, submits[0][-1])
                self.assertTrue(any("--immutable-tags" in args for args in calls))
            self.assertEqual(output.getvalue().strip(), "us-west1-docker.pkg.dev/family-finance-trial/wealthfolio/server@sha256:" + "a" * 64)

    def vm(self):
        return {"id": "123", "metadata": {"items": []}, "serviceAccounts": [{"email": "runtime@example.com", "scopes": ["https://www.googleapis.com/auth/cloud-platform"]}],
                "networkInterfaces": [{"network": "https://compute.googleapis.com/projects/family-finance-trial/global/networks/default", "accessConfigs": [{"natIP": "192.0.2.1"}]}]}

    def install_context(self, config=None):
        path = self.root / "config.json"
        path.write_text(json.dumps(config or self.config))
        return self.args(config=path)

    def test_install_rejects_startup_automation_identity_and_dns_before_cutover(self):
        cases = []
        vm = self.vm(); vm["metadata"]["items"] = [{"key": "startup-script", "value": "private"}]; cases.append(vm)
        vm = self.vm(); vm["serviceAccounts"] = []; cases.append(vm)
        vm = self.vm(); vm["serviceAccounts"][0]["scopes"] = []; cases.append(vm)
        vm = self.vm(); vm["networkInterfaces"][0]["accessConfigs"] = []; cases.append(vm)
        vm = self.vm(); vm["networkInterfaces"][0]["accessConfigs"][0]["natIP"] = "192.0.2.2"; cases.append(vm)
        for vm in cases:
            with self.subTest(vm=vm), patch.object(manage, "resource", return_value=vm), patch.object(manage.socket, "getaddrinfo", return_value=[(0, 0, 0, "", ("192.0.2.1", 443))]), patch.object(manage, "gcloud") as cloud, patch.object(manage, "ssh") as ssh:
                with self.assertRaises(manage.DeploymentError):
                    manage.install(self.install_context())
                cloud.assert_not_called()
                ssh.assert_not_called()

    def test_install_limits_grants_and_metadata_and_reuses_proxy_ownership(self):
        for proxy, firewall in [(True, None), (True, {}), (False, None)]:
            config = dict(self.config, manage_proxy=proxy)
            metadata = []
            def cloud(arguments, project, **kwargs):
                if "add-metadata" in arguments:
                    paths = arguments[-1].split("=", 1)[1].split(",")
                    metadata.append([json.loads(Path(item.split("=", 1)[1]).read_text()) for item in paths])
            output = io.StringIO()
            with self.subTest(proxy=proxy, firewall=firewall), patch.object(manage, "resource", side_effect=[self.vm(), firewall] if proxy else [self.vm()]), patch.object(manage.socket, "getaddrinfo", return_value=[(0, 0, 0, "", ("192.0.2.1", 443))]), patch.object(manage, "gcloud", side_effect=cloud) as writes, patch.object(manage, "ssh", return_value=b"WEALTHFOLIO_BOOTSTRAP_READY") as ssh, contextlib.redirect_stdout(output):
                manage.install(self.install_context(config))
            calls = [call.args[0] for call in writes.call_args_list]
            self.assertEqual(sum(args[:2] == ["secrets", "add-iam-policy-binding"] for args in calls), 3)
            self.assertEqual(any(args[:3] == ["compute", "firewall-rules", "create"] for args in calls), proxy and firewall is None)
            self.assertEqual(metadata[0][0], config)
            self.assertIn("release.py", metadata[0][1])
            self.assertNotIn("secretAccessor", ssh.call_args.args[-1])
            self.assertIn(config["domain"], output.getvalue())

    def test_install_shared_vpc_and_missing_ready_marker_do_not_report_success(self):
        vm = self.vm(); vm["networkInterfaces"][0]["network"] = "/projects/other/global/networks/shared"
        with patch.object(manage, "resource", return_value=vm), patch.object(manage.socket, "getaddrinfo", return_value=[(0, 0, 0, "", ("192.0.2.1", 443))]), patch.object(manage, "gcloud"), patch.object(manage, "ssh") as ssh:
            with self.assertRaisesRegex(manage.DeploymentError, "Shared-VPC"):
                manage.install(self.install_context())
            ssh.assert_not_called()
        with patch.object(manage, "resource", return_value=self.vm()), patch.object(manage, "gcloud"), patch.object(manage, "ssh", return_value=b"not-ready"), contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaisesRegex(manage.DeploymentError, "did not report"):
                manage.install(self.install_context(dict(self.config, manage_proxy=False)))
            self.assertEqual(output.getvalue(), "")

    def test_operator_cli_dispatches_and_redacts_unexpected_dependency_errors(self):
        for operation, extra in [("inspect", ["--instance", "wealthfolio"]), ("seed", []), ("build", []), ("install", ["--instance", "wealthfolio", "--config", "unused"])]:
            argv = ["manage.py", "--project", "family-finance-trial", operation, *extra]
            with self.subTest(operation=operation), patch.object(sys, "argv", argv), patch.object(manage, operation) as action, patch.object(manage.os, "chdir"):
                self.assertEqual(manage.main(), 0)
                action.assert_called_once()
        for error in [OSError("private"), manage.DeploymentError("safe explanation")]:
            with patch.object(sys, "argv", ["manage.py", "--project", "family-finance-trial", "build"]), patch.object(manage, "build", side_effect=error), patch.object(manage.os, "chdir"), contextlib.redirect_stderr(io.StringIO()) as output:
                self.assertEqual(manage.main(), 1)
                self.assertNotIn("private", output.getvalue())
        for argv in [["--project", "bad;project", "build"], ["--project", "family-finance-trial", "inspect", "--instance", "invalid name"]]:
            with patch.object(sys, "argv", ["manage.py", *argv]), contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                manage.main()


class BootstrapTests(unittest.TestCase):
    args = release_tests.TrustTests.args

    def vm(self):
        return {"networkInterfaces": [{"network": "/projects/family-finance-trial/global/networks/default"}],
                "metadata": {"items": [{"key": "enable-oslogin", "value": "TRUE"}]}, "serviceAccounts": [{"email": self.args().runtime_account}]}

    def test_bootstrap_preflight_rejects_wrong_project_network_identity_and_mutable_repository(self):
        cases = []
        vm = self.vm(); cases.append((vm, {"projectNumber": "wrong"}, {}))
        vm = self.vm(); vm["networkInterfaces"][0]["network"] = "/projects/other/global/networks/shared"; cases.append((vm, {"projectNumber": "123456789"}, {}))
        vm = self.vm(); vm["metadata"] = {}; cases.append((vm, {"projectNumber": "123456789"}, {}))
        vm = self.vm(); vm["serviceAccounts"] = []; cases.append((vm, {"projectNumber": "123456789"}, {}))
        cases.append((self.vm(), {"projectNumber": "123456789"}, {"dockerConfig": {"immutableTags": False}}))
        for vm, project, repo in cases:
            with self.subTest(vm=vm), patch.object(bootstrap.manage, "resource", side_effect=[vm, project, repo]), patch.object(bootstrap.manage, "gcloud") as write:
                with self.assertRaises(ValueError):
                    bootstrap.apply(self.args(), bootstrap.plan(self.args())[0])
                write.assert_not_called()

    def test_bootstrap_creates_or_reconciles_only_dedicated_resources(self):
        for exists in [False, True]:
            def resource(arguments, project, **kwargs):
                if arguments[:3] == ["compute", "instances", "describe"]:
                    return self.vm()
                if arguments[:2] == ["projects", "describe"]:
                    return {"projectNumber": "123456789"}
                if arguments[:3] == ["artifacts", "repositories", "describe"]:
                    return {"dockerConfig": {"immutableTags": True}}
                if arguments[:3] == ["compute", "firewall-rules", "describe"] and exists:
                    return {"sourceRanges": ["35.235.240.0/20"], "targetTags": [arguments[3]], "allowed": [{"IPProtocol": "tcp", "ports": ["22"]}]}
                return {"name": "existing"} if exists else None
            with self.subTest(exists=exists), patch.object(bootstrap.manage, "resource", side_effect=resource), patch.object(bootstrap.manage, "gcloud") as write:
                bootstrap.apply(self.args(), bootstrap.plan(self.args())[0])
            calls = [call.args[0] for call in write.call_args_list]
            self.assertEqual(any("create-oidc" in args for args in calls), not exists)
            self.assertEqual(any("update-oidc" in args for args in calls), exists)
            self.assertEqual(any(args[:3] == ["compute", "firewall-rules", "create"] for args in calls), not exists)
            self.assertFalse(any(args[:3] == ["compute", "instances", "set-service-account"] for args in calls))
            self.assertTrue(any("--permissions=compute.projects.get,serviceusage.services.use" in args for args in calls))

    def test_conflicting_iap_firewall_is_rejected_without_adding_instance_tag(self):
        for override in [{"sourceRanges": ["0.0.0.0/0"]}, {"targetTags": ["other"]}, {"allowed": [{"IPProtocol": "tcp", "ports": ["443"]}]}]:
            def resource(arguments, project, **kwargs):
                if arguments[:3] == ["compute", "instances", "describe"]: return self.vm()
                if arguments[:2] == ["projects", "describe"]: return {"projectNumber": "123456789"}
                if arguments[:3] == ["artifacts", "repositories", "describe"]: return {"dockerConfig": {"immutableTags": True}}
                return dict({"sourceRanges": ["35.235.240.0/20"], "targetTags": [arguments[3]], "allowed": [{"IPProtocol": "tcp", "ports": ["22"]}]}, **override)
            with self.subTest(override=override), patch.object(bootstrap.manage, "resource", side_effect=resource), patch.object(bootstrap.manage, "gcloud") as write:
                with self.assertRaisesRegex(ValueError, "firewall rule differs"):
                    bootstrap.apply(self.args(), [["ensure-iap-firewall"]])
                write.assert_not_called()

    def test_bootstrap_validates_all_external_identifiers(self):
        for key, value in [("project", "bad;id"), ("project_number", "NaN"), ("instance", "--bad"), ("region", "us west"), ("runtime_account", "other@example.com")]:
            args = self.args(); setattr(args, key, value)
            with self.subTest(key=key), self.assertRaises(ValueError): bootstrap.plan(args)
        args = self.args(); args.runtime_account = "123456789-compute@developer.gserviceaccount.com"
        self.assertEqual(bootstrap.plan(args)[1]["WF_GCP_PROJECT"], "family-finance-trial")
        for ids in [("bad", "123", "build"), ("123", "bad", "build"), ("123", "456", "other")]:
            with self.subTest(ids=ids), self.assertRaises(ValueError): bootstrap.trust_condition(*ids)

    def test_bootstrap_cli_is_a_dry_run_by_default_and_redacts_failures(self):
        argv = ["bootstrap-ci.py", "--project-number", "123456789", "--owner-id", "12345", "--runtime-account", self.args().runtime_account]
        for apply in [False, True]:
            with self.subTest(apply=apply), patch.object(sys, "argv", argv + (["--apply"] if apply else [])), patch.object(bootstrap, "apply") as change, contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(bootstrap.main(), 0)
                self.assertEqual(change.called, apply)
                self.assertEqual(json.loads(output.getvalue())["github_variables"]["WF_GCP_RELEASE_ENABLED"], "false")
        with patch.object(sys, "argv", argv + ["--apply"]), patch.object(bootstrap, "apply", side_effect=OSError("private")), contextlib.redirect_stderr(io.StringIO()) as output:
            self.assertEqual(bootstrap.main(), 1)
            self.assertNotIn("private", output.getvalue())
