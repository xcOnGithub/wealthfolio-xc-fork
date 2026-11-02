import argparse
import importlib.util
import json
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch, MagicMock

HERE = Path(__file__).parent


def module(name, file):
    spec = importlib.util.spec_from_file_location(name, HERE / file)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


release = module("release", "release.py")
bootstrap = module("bootstrap_ci", "bootstrap-ci.py")
gate = module("production_gate", "check-production-environment.py")
BASE = "us-west1-docker.pkg.dev/example-finance-project/wealthfolio/server@sha256:"
RUN = "https://github.com/xcOnGithub/wealthfolio-private/actions/runs/123/attempts/1"


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.data = self.root / "data"
        self.data.mkdir()
        (self.data / "web-account.json").write_text("original-owner")
        self.config_path = self.root / "config.json"
        self.config = {"image": BASE + "a" * 64, "data_dir": str(self.data), "secrets": {"master_key": "pinned-version"}}
        self.config_path.write_text(json.dumps(self.config))
        self.records = {
            BASE + "a" * 64: {"org.opencontainers.image.revision": "1" * 40, "org.wealthfolio.state-fingerprint": "a" * 64, "org.wealthfolio.config-contract": release.CONTRACT},
            BASE + "b" * 64: {"org.opencontainers.image.revision": "2" * 40, "org.wealthfolio.state-fingerprint": "a" * 64, "org.wealthfolio.config-contract": release.CONTRACT},
        }
        self.commands = []
        self.owner = patch.object(release.runtime.os, "fchown")
        self.owner.start()
        self.addCleanup(self.owner.stop)
        self.manager = release.Releases(self.config_path, self.root / "history", run=self.fake_run, health=lambda *_: None, prepare=lambda *_: None)
        self.manager.initialize()

    def fake_run(self, arguments, **options):
        self.commands.append(arguments)
        if arguments[:3] == ["docker", "image", "inspect"]:
            return json.dumps(self.records[arguments[-1]])
        return ""

    def request(self, operation="deploy", digest="b"):
        return {"operation": operation, "image": BASE + digest * 64, "commit": "2" * 40, "run_url": RUN}

    def test_release_changes_only_image_and_records_provenance(self):
        state = self.manager.apply(self.request())
        actual = json.loads(self.config_path.read_text())
        self.assertEqual(actual, dict(self.config, image=BASE + "b" * 64))
        self.assertEqual(state["current"]["commit"], "2" * 40)
        self.assertEqual(state["current"]["run_url"], RUN)
        self.assertTrue(Path(state["current"]["backup"]).exists())
        self.assertEqual((self.data / "web-account.json").read_text(), "original-owner")

    def test_unknown_fields_and_foreign_images_fail_before_commands(self):
        cases = [dict(self.request(), secrets={}), dict(self.request(), image="attacker/image:latest"),
                 dict(self.request(), commit="main"), dict(self.request(), run_url="https://attacker.example/run"), []]
        for request in cases:
            with self.subTest(request=request):
                self.commands.clear()
                with self.assertRaises(ValueError):
                    self.manager.apply(request)
                self.assertEqual(self.commands, [])
        self.assertEqual(json.loads(self.config_path.read_text()), self.config)

    def test_wrong_commit_and_missing_labels_do_not_stop_service(self):
        with self.assertRaises(ValueError):
            self.manager.apply(dict(self.request(), commit="3" * 40))
        self.records[BASE + "b" * 64].pop("org.wealthfolio.config-contract")
        with self.assertRaises(ValueError):
            self.manager.apply(self.request())
        self.assertNotIn(["systemctl", "stop", "wealthfolio.service"], self.commands)

    def test_compatible_failure_rolls_back_image_without_rewinding_password_state(self):
        def health(config, image):
            if image == BASE + "b" * 64:
                (self.data / "web-account.json").write_text("new-password-state")
                raise RuntimeError("synthetic failed health check")
        self.manager.health = health
        self.manager.wait_healthy = health
        with self.assertRaisesRegex(RuntimeError, "previous compatible image"):
            self.manager.apply(self.request())
        self.assertEqual(json.loads(self.config_path.read_text()), self.config)
        self.assertEqual((self.data / "web-account.json").read_text(), "new-password-state")
        state = self.manager.state()
        self.assertTrue(state["failed"][-1]["automatic_image_rollback"])
        self.assertEqual(state["current"]["image"], self.config["image"])

    def test_possible_state_change_stops_and_blocks_more_releases(self):
        self.records[BASE + "b" * 64]["org.wealthfolio.state-fingerprint"] = "b" * 64
        self.manager.wait_healthy = lambda *_: (_ for _ in ()).throw(RuntimeError("unhealthy"))
        with self.assertRaisesRegex(RuntimeError, "manual recovery"):
            self.manager.apply(self.request())
        self.assertEqual(self.commands[-1], ["systemctl", "stop", "wealthfolio.service"])
        state = self.manager.apply({"operation": "status"})
        self.assertTrue(state["recovery_required"])
        with self.assertRaises(ValueError):
            self.manager.apply(self.request())
        self.assertEqual((self.data / "web-account.json").read_text(), "original-owner")

    def test_timed_out_candidate_uses_the_same_compatible_rollback_path(self):
        def health(config, image):
            if image == BASE + "b" * 64:
                raise release.subprocess.TimeoutExpired("docker", 600)
        self.manager.wait_healthy = health
        with self.assertRaisesRegex(RuntimeError, "previous compatible image"):
            self.manager.apply(self.request())
        self.assertEqual(json.loads(self.config_path.read_text()), self.config)
        self.assertTrue(self.manager.state()["failed"][-1]["automatic_image_rollback"])

    def test_manual_rollback_requires_known_success_and_equal_state_fingerprint(self):
        with self.assertRaisesRegex(ValueError, "recorded successful"):
            self.manager.apply(self.request("rollback"))
        self.manager.apply(self.request())
        self.records[BASE + "a" * 64]["org.wealthfolio.state-fingerprint"] = "f" * 64
        with self.assertRaises(ValueError):
            self.manager.apply(self.request("rollback", "a"))
        self.records[BASE + "a" * 64]["org.wealthfolio.state-fingerprint"] = "a" * 64
        state = self.manager.apply(self.request("rollback", "a"))
        self.assertEqual(state["current"]["commit"], "1" * 40)

    def test_backup_failure_restarts_original_without_changing_config(self):
        with patch.object(release, "backup_data", side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                self.manager.apply(self.request())
        self.assertEqual(self.commands[-1], ["systemctl", "start", "wealthfolio.service"])
        self.assertEqual(json.loads(self.config_path.read_text()), self.config)

    def test_history_drift_requires_operator_reconciliation(self):
        self.config_path.write_text(json.dumps(dict(self.config, image=BASE + "b" * 64)))
        with self.assertRaisesRegex(ValueError, "history"):
            self.manager.apply({"operation": "status"})

    def test_baseline_does_not_erase_existing_release_history(self):
        self.manager.apply(self.request())
        self.manager.initialize()
        self.assertEqual(len(self.manager.state()["successful"]), 2)

    def test_initialization_retries_transient_https_failure_before_saving_history(self):
        history = self.manager.state_dir / "history.json"
        history.unlink()
        attempts = []
        def health(*_):
            self.assertFalse(history.exists())
            attempts.append(True)
            if len(attempts) == 1:
                raise OSError("TLS certificate is not ready")
        self.manager.health = health
        with patch.object(release.time, "sleep") as sleep:
            self.manager.initialize()
        self.assertEqual(len(attempts), 2)
        sleep.assert_called_once_with(2)
        self.assertTrue(history.exists())

    def test_exhausted_initialization_does_not_create_release_history(self):
        history = self.manager.state_dir / "history.json"
        history.unlink()
        self.manager.health = lambda *_: (_ for _ in ()).throw(OSError("TLS unavailable"))
        with patch.object(release.time, "monotonic", side_effect=[0, 180]):
            with self.assertRaisesRegex(RuntimeError, "verification failed"):
                self.manager.initialize()
        self.assertFalse(history.exists())


class TrustTests(unittest.TestCase):
    def args(self):
        return argparse.Namespace(project="example-finance-project", project_number="123456789", repository_id="1437991235", owner_id="12345",
                                  instance="wealthfolio", region="us-west1", zone="us-west1-b", runtime_account="wealthfolio-runtime@example-finance-project.iam.gserviceaccount.com")

    def test_trust_binds_repository_owner_workflow_ref_environment_and_event(self):
        commands, variables = bootstrap.plan(self.args())
        providers = [entry for entry in commands if entry[0] == "ensure-provider"]
        self.assertEqual(len(providers), 2)
        for entry in providers:
            condition = entry[-1]
            for field in ["assertion.repository_id", "assertion.repository_owner_id", "assertion.workflow_ref", "assertion.ref", "assertion.environment", "assertion.event_name"]:
                self.assertIn(field, condition)
            self.assertIn("refs/heads/main", condition)
            self.assertIn("gcp-release.yml@refs/heads/main", condition)
        self.assertNotEqual(providers[0][-1], providers[1][-1])
        self.assertEqual(variables["WF_GCP_RELEASE_ENABLED"], "false")
        flattened = json.dumps(commands)
        for forbidden in ["roles/editor", "roles/owner", "roles/secretmanager.secretAccessor", "roles/iam.serviceAccountTokenCreator", "roles/resourcemanager.projectIamAdmin"]:
            self.assertNotIn(forbidden, flattened)

    def test_bootstrap_rejects_injected_ids_and_foreign_runtime_identity(self):
        for field, value in [("owner_id", "123' || true"), ("repository_id", "anything"), ("runtime_account", "foreign@other-project.iam.gserviceaccount.com"), ("zone", "us-east1-b")]:
            with self.subTest(field=field):
                args = self.args()
                setattr(args, field, value)
                with self.assertRaises(ValueError):
                    bootstrap.plan(args)

    def test_production_gate_requires_reviewers_and_branch_restriction(self):
        environment = {"protection_rules": [{"type": "required_reviewers", "reviewers": [{"id": 1}]}], "deployment_branch_policy": {"custom_branch_policies": True}}
        gate.validate(environment, {"branch_policies": [{"name": "main", "type": "branch"}]})
        with self.assertRaises(ValueError):
            gate.validate(dict(environment, protection_rules=[]), {})
        with self.assertRaises(ValueError):
            gate.validate(environment, {"branch_policies": [{"name": "*", "type": "branch"}]})

    def test_federation_files_are_excluded_from_all_source_contexts(self):
        root = HERE.parents[1]
        for name in [".gitignore", ".dockerignore", ".gcloudignore"]:
            self.assertIn("gha-creds-*.json", (root / name).read_text())


class VerificationTests(unittest.TestCase):
    def response(self, value=None):
        response = MagicMock()
        body = io.BytesIO(json.dumps(value).encode())
        response.__enter__.return_value = body
        body.status = 200
        return response

    def test_verification_checks_both_origins_and_rejects_anonymous_sessions(self):
        denied = release.urllib.error.HTTPError("url", 401, "denied", {}, None)
        responses = []
        for _ in range(2):
            responses.extend([self.response(), self.response({"accountAuth": True, "requiresPassword": True}), denied])
        with patch.object(release.urllib.request, "urlopen", side_effect=responses) as get:
            release.verify({"bind_port": 8088, "domain": "app.example.com"}, BASE + "a" * 64, run=lambda *_: "same-image")
        urls = [call.args[0] for call in get.call_args_list]
        self.assertEqual(len(urls), 6)
        self.assertIn("https://app.example.com/api/v1/auth/me", urls)
        self.assertIn("http://127.0.0.1:8088/api/v1/healthz", urls)

    def test_running_image_mismatch_fails_before_network_requests(self):
        values = iter(["expected", "unexpected"])
        with patch.object(release.urllib.request, "urlopen") as get:
            with self.assertRaisesRegex(ValueError, "selected image"):
                release.verify({"bind_port": 8088, "domain": "app.example.com"}, BASE + "a" * 64, run=lambda *_: next(values))
            get.assert_not_called()

    def test_disabled_account_auth_and_anonymous_success_fail_verification(self):
        config = {"bind_port": 8088, "domain": "app.example.com"}
        cases = [
            [self.response(), self.response({"accountAuth": False, "requiresPassword": True})],
            [self.response(), self.response({"accountAuth": True, "requiresPassword": False})],
            [self.response(), self.response({"accountAuth": True, "requiresPassword": True}), self.response()],
        ]
        for responses in cases:
            with self.subTest(), patch.object(release.urllib.request, "urlopen", side_effect=responses):
                with self.assertRaises(ValueError):
                    release.verify(config, BASE + "a" * 64, run=lambda *_: "same-image")


if __name__ == "__main__":
    unittest.main()
