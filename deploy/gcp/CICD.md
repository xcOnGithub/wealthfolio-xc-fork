# GitHub Actions releases to the existing GCP VM

Examples use the placeholder project `example-finance-project`, instance
`YOUR_INSTANCE`, and zone `us-west1-b` (region `us-west1`). Supply your own
values. This package supplies the tooling; merging it does not configure IAM,
create Secret Manager versions, or enable deployments. Keep the repository
variable `WF_GCP_RELEASE_ENABLED=false` until the setup below is complete.

## Release sequence

1. A push to `main` or a manual run on `main` runs server tests, Clippy,
   formatting, frontend tests, lint, types, scripts, theme validation, and the
   web build. Deployment unit tests and the owner-auth coverage gate are also
   required.
2. The workflow verifies the production environment has required reviewers and a
   branch restriction. An unavailable GitHub API or missing protection fails
   closed before cloud authentication. Private repositories need a GitHub plan
   that supports required environment reviewers; confirm that before enabling.
3. The build job builds one `linux/amd64` image without GCP credentials.
   Disposable image tests check encryption, file-based keys, CA trust,
   signup/login, private access, and owner persistence after restart. They use
   synthetic credentials and never request recovery email.
4. The builder exchanges GitHub OIDC for a short-lived token and pushes to
   private Artifact Registry. A unique immutable tag includes commit, run, and
   attempt; deployment uses the resulting digest, not a tag.
5. A production reviewer approves the deployment. The deployer then
   authenticates through OIDC and calls the installed release command over
   IAP/OS Login. It sends only an image digest, source commit, operation, and
   Actions run URL.
6. The VM validates labels, pulls the candidate, verifies the original master
   key and existing volume, stops the service, creates an offline backup, and
   changes only the image in its installed configuration. It checks local and
   public HTTPS health, required owner authentication, anonymous rejection, and
   running image identity before recording success.

Workflow concurrency queues releases without cancelling an active deployment. A
host file lock serializes CI with manual operations. GitHub artifacts contain
coverage and release metadata only; backups remain on the VM.

## Identity boundaries

| Identity                    | Grants                                                                                                                                                     |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| GitHub builder              | Writer on the one Artifact Registry repository                                                                                                             |
| GitHub deployer             | Reader on that repository, OS Admin Login on this VM, IAP tunnel access to this instance on port 22, actAs on the VM account, project lookup/service usage |
| Existing VM runtime account | Reader on that repository and Secret Accessor on exactly the three runtime secrets                                                                         |
| Setup operator              | Temporary administrative permissions for APIs, IAM, federation, firewall, secrets, and initial installation                                                |

Normal releases do not run `manage.py install` or `bootstrap-ci.py`. They do not
modify IAM or copy secret values through GitHub. OS Admin Login nevertheless
grants root authority on the VM: a compromised deployer can read runtime
secrets. Protect `main`, workflow changes, environment approvals, and repository
admins accordingly. The runtime account's pre-existing grants must also be
audited.

The dedicated WIF pool has separate build/production providers. Each checks the
numeric repository and owner IDs, exact workflow path, `refs/heads/main`, its
GitHub environment, and push/manual event type. Forks and PR events cannot use
these identities. Keep the pool dedicated; other providers must not be allowed
to assert the same pipeline-role attributes.

The deployment pins `xcOnGithub/wealthfolio-xc-fork`. GitHub redirects from a
previous repository name do not update OIDC workflow claims or release URL
validation. If federation or the VM release command was installed before a
rename, reconcile the providers with the one-time operator bootstrap and update
the root-installed release tooling while preserving the current image, data,
secret references, and release history. Keep releases disabled until reconciled.

## One-time setup

1. Complete the inspection and credential migration in [README.md](README.md).
   Confirm the VM is dedicated, its public domain/DNS/TLS, existing data mount,
   original master key, current proxy, and operator recovery access. Import the
   actual installation credentials into three pinned Secret Manager versions.
   Local test keys must not replace an existing installation's key.
2. Verify the VM runtime identity and `cloud-platform` scope. Enable OS Login
   only after testing independent operator recovery access. The CI bootstrap
   requires instance metadata `enable-oslogin=TRUE`; it deliberately refuses to
   change SSH authentication or the attached identity automatically.
3. Build and install a clean committed baseline using `manage.py build/install`.
   The immutable-tag `wealthfolio` Docker repository must already exist. The
   baseline must carry revision, config-contract, and backend fingerprint
   labels. Bootstrap installs `/opt/wealthfolio/release.py` as root and
   initializes `/var/lib/wealthfolio-release/history.json` only after HTTPS/auth
   verification. Updating tooling must preserve existing history and the current
   image.
4. Resolve and verify identifiers using the authenticated operator SDK and
   GitHub CLI. These outputs are identifiers, not credentials:

   ```sh
   gcloud projects describe example-finance-project --format='value(projectNumber)'
   gcloud compute instances describe YOUR_INSTANCE --project example-finance-project \
     --zone us-west1-b --format='value(serviceAccounts.email)'
   gh api repos/xcOnGithub/wealthfolio-xc-fork --jq '{repository_id: .id, owner_id: .owner.id}'
   ```

5. Review the non-mutating federation/IAM plan:

   ```sh
   python3 deploy/gcp/bootstrap-ci.py \
     --project YOUR_PROJECT_ID --instance YOUR_INSTANCE \
     --project-number PROJECT_NUMBER --repository-id REPOSITORY_ID \
     --owner-id OWNER_ID --runtime-account EXISTING_VM_ACCOUNT
   ```

   Run the same command with `--apply` as the setup operator after review. It
   configures the dedicated identities/providers, scoped bindings, lookup role,
   and VM-specific IAP SSH firewall rule. It verifies project/VM identity and
   repository immutability first. Shared VPC setup requires its network owner.
   Existing public SSH firewall rules are not removed; audit them separately.
   Re-running reconciles these named resources; use a dedicated pool/accounts.

6. In GitHub, create environments `build` and `production`. Restrict both to
   `main`. Give `production` at least one required reviewer; enable prevention
   of self-review where the team's approval model permits it. Protect `main` and
   require CI before merge. Configure the nonsecret repository variables printed
   by bootstrap: `WF_GCP_PROJECT`, `WF_GCP_INSTANCE`, `WF_GCP_ZONE`,
   `WF_GCP_IMAGE`, `WF_GCP_BUILD_PROVIDER`, `WF_GCP_DEPLOY_PROVIDER`,
   `WF_GCP_BUILD_ACCOUNT`, `WF_GCP_DEPLOY_ACCOUNT`, and initially
   `WF_GCP_RELEASE_ENABLED=false`. Preserve any required public `CONNECT_*`
   build variables. No GCP key JSON, Resend API key, master key, or invitation
   is stored in GitHub.
7. Verify VM egress, IAP connectivity, Secret Manager access, TLS, and
   root-owned installation/config/history permissions. Set
   `WF_GCP_RELEASE_ENABLED=true` only when ready. Run the release workflow on
   `main`, review the image digest, approve production, and verify sign-in and
   recovery email delivery manually. Reboot once to verify secret fetch and
   systemd startup. Retain the approval requirement; removing it requires a
   reviewed workflow and trust-policy change.

## Rollback and recovery

Dispatch the release workflow on `main` with `rollback_digest` set to a complete
immutable image reference in successful release history. Validation and
production approval still apply; no new image is built. The selected image must
retain its recorded labels and have the same backend-state fingerprint as the
current image.

The fingerprint covers tracked server/crate sources, Cargo manifests/lockfile,
toolchain, Dockerfile, and config. It is deliberately conservative: even a
backend test or dependency change can prohibit automatic image rollback. It is
not a schema compatibility proof. Backend changes require review before forward
deployment and a manual recovery decision if startup fails.

A failed compatible candidate restores only the previous image and
configuration; it never restores `web-account.json`, passwords, recovery state,
or financial data. A failed candidate with a different fingerprint stops the
service, records `recovery_required`, and blocks further releases. An operator
must inspect the journal, installed configuration, history, and root-only
backup, then decide whether a forward fix or offline restoration is appropriate.
Reconcile history and configuration after recovery. Never restore account state
automatically or erase release history to bypass this guard.

Backups are compressed archives under `/var/lib/wealthfolio-release/backups`,
mode `0600` in root-only directories. They contain sensitive installation data.
They are on the same VM and do not replace separately protected
disaster-recovery backups. Monitor disk usage and establish retention before
repeated production releases; insufficient space blocks cutover.

## Coverage scope and validation limits

The reusable owner-auth job enforces **both** 95% combined line and branch
coverage on executable code added by the owner-auth feature since its fixed
baseline `669418ccea9b15212abb81bc8ce3db0fbd18c646`, across four frontend auth
files, three server auth/config files, and the invitation script. New deployment
key-file lines in those files are included. This is not repository-wide
coverage. Reports combine V8 and LLVM source-location definitions and fail on a
missing scoped report.

Deployment Python has its own **95% combined line and 95% combined branch** gate
in both the PR deployment check and reusable release validation. Coverage.py is
pinned in `requirements-test.txt`. All Python scripts under `deploy/gcp` are
included, including operator commands, CLI entrypoints, and the image-smoke
helper; only `test_*.py` files are omitted. Missing script reports or missing
instrumentation fail closed. The gate checks lines and branches independently
and retains the JSON report as a 14-day GitHub artifact. Shell, embedded shell
Python, workflow YAML, and real cloud/image behavior are outside this Python
unit-coverage measurement.

Rust branch coverage uses pinned experimental nightly instrumentation. The
profiling run excludes one existing subprocess test that deliberately clears its
environment and causes LLVM to create a profile in an expected-empty directory.
The ordinary server validation job runs that test without exclusion. Inline Rust
tests and the test-only reset helper are excluded from the coverage denominator.
LLVM export is limited to the three scoped files to avoid an unrelated exporter
crash. Frontend coverage uses the three focused auth suites; separate jobs run
the full frontend suite. Reports are retained as GitHub artifacts.

Local unit tests use temporary data and mocked cloud/system commands. They cover
request restrictions, WIF conditions, reviewer protection, provenance, backups,
image rollback without password rewind, incompatible failures, and history
drift. Additional tests cover IAM reconciliation and preflight failures, SDK and
CLI redaction, credential export, atomic-write cleanup, backup contents and disk
space, master-key/volume drift, and image-smoke orchestration/cleanup. The
latter uses mocked Docker and HTTP boundaries; it does not execute an
application image. Live federation, IAM grants, actual image build/smoke
execution, Secret Manager access, and VM rollout must pass in the configured
project before deployment is verified.

Architecture impact: adds an image release history, host serialization, offline
backups, and synchronous deployment health checks around existing
Compose/systemd. The history prevents unrecorded rollback; the lock prevents
overlapping cutovers. The only application configuration extension is a
read-once Resend key-file input. Account/financial persistence formats and
provider request behavior remain unchanged. Runtime secrets remain VM-owned; CI
only transmits release identifiers.
