"""Exercise GHCR owner normalization and cache producer/consumer wiring."""
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest


WORKFLOWS = Path(__file__).resolve().parents[1] / "workflows"
JOBS = [("cache-warm.yml", "warm-docker-deps"),
        ("docker-publish.yml", "build"), ("docker-publish.yml", "publish")]


def job_text(workflow, job):
    text = (WORKFLOWS / workflow).read_text()
    match = re.search(rf"(?ms)^  {re.escape(job)}:\n(.*?)(?=^  [\w-]+:|\Z)", text)
    if not match:
        raise AssertionError(f"Missing {workflow}/{job}")
    return match.group(1)


class GhcrWorkflowTests(unittest.TestCase):
    def test_real_normalization_steps_accept_mixed_case_and_lowercase_owners(self):
        for workflow, job in JOBS:
            text = job_text(workflow, job)
            step = re.search(r"(?ms)^      - name: Normalize GHCR owner\n(.*?)(?=^      - |\Z)", text)
            self.assertIsNotNone(step, f"{workflow}/{job}")
            script = re.search(r"^        run: (.+)$", step.group(1), re.M).group(1)
            for owner in ["xcOnGithub", "lowercase-org", "Mixed-Case99"]:
                with self.subTest(workflow=workflow, job=job, owner=owner), tempfile.TemporaryDirectory() as root:
                    output = Path(root) / "output"
                    result = subprocess.run(
                        ["bash", "-e", "-u", "-c", script],
                        env=dict(os.environ, GITHUB_REPOSITORY_OWNER=owner, GITHUB_OUTPUT=str(output)),
                        capture_output=True, text=True,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(output.read_text(), f"owner={owner.lower()}\n")

    def test_cache_and_release_refs_consume_normalized_owner(self):
        warmer = re.sub(r"\s+", " ", job_text("cache-warm.yml", "warm-docker-deps"))
        builder = re.sub(r"\s+", " ", job_text("docker-publish.yml", "build"))
        publisher = re.sub(r"\s+", " ", job_text("docker-publish.yml", "publish"))
        for text in [warmer, builder, publisher]:
            self.assertIn("id: ghcr", text)
            self.assertNotIn("github.repository_owner", text)
        self.assertEqual(warmer.count("ghcr.io/${{ steps.ghcr.outputs.owner }}/wealthfolio-build-cache"), 2)
        self.assertIn("github.ref == 'refs/heads/main'", warmer)
        self.assertIn("steps.ghcr.outputs.owner, matrix.arch", builder)
        self.assertIn("!startsWith(github.ref, 'refs/tags/')", builder)
        for text in [builder, publisher]:
            self.assertIn("ghcr.io/${{ steps.ghcr.outputs.owner }}/wealthfolio", text)


if __name__ == "__main__":
    unittest.main()
