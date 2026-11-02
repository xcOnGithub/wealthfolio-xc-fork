# Existing GCP VM deployment

This package upgrades one existing Wealthfolio container on a Compute Engine VM.
It preserves the selected `/data` mount, database filename, installation
account, and master key. It does not create a new VM, format disks, generate a
replacement master key, or overwrite existing Secret Manager versions.

It targets a dedicated Wealthfolio VM. Other application containers require a
separate metadata-isolation/network plan; the preflight refuses them rather than
applying a global Docker metadata rule to unrelated workloads.

Examples use the placeholder project `example-finance-project`, instance
`YOUR_INSTANCE`, zone `us-west1-b`, and region `us-west1`; replace them with
your own deployment values. The public domain, existing data mount, and proxy
ownership must be established by inspection before deployment. For recurring
GitHub Actions releases, follow [the CI/CD setup and recovery runbook](CICD.md).

## Prerequisites

- An authenticated Google Cloud CLI on the operator machine. Use its configured
  identity or an attached identity; do not create service-account key files.
- Permission to inspect the selected VM and use IAP SSH, manage the three named
  Secret Manager secrets and their IAM bindings, and build/push an Artifact
  Registry image. The build command also creates a dedicated Cloud Build service
  account and source-staging bucket. Billing must support Cloud Build.
- The VM's existing Docker Engine, Compose v2, Python 3, systemd, and iptables.
  This package deliberately does not upgrade the VM's Docker installation.
- A VM service account with the `cloud-platform` OAuth scope. Its existing IAM
  grants must be audited: adding narrow secret grants does not remove old broad
  grants. This tool does not replace the VM identity or stop the VM to change
  scopes, because that can affect other applications.
- Network access from the operator to Compute, IAM, Secret Manager, Cloud Build,
  Artifact Registry, Cloud Storage, Service Usage, OS Login, and IAP APIs. From
  the VM: metadata, Secret Manager, its image registry, Docker Hub, and the
  application's HTTPS providers, including `api.resend.com`.

## Inspect first

```sh
python3 deploy/gcp/manage.py --project example-finance-project inspect \
  --instance YOUR_INSTANCE --zone us-west1-b
```

Inventory output contains image references, mounts, ports, public settings, and
credential-input **names**, not their values. Inspect existing startup scripts,
systemd units, reverse proxies, and other workloads before cutover. The
installer refuses VMs with legacy startup-script/container-declaration metadata
until that automation is audited and migrated. It leaves unrelated workloads and
VM identity configuration alone.

## Import the existing installation credentials into Secret Manager

For an existing installation, import its actual VM master key. A local test
master key may differ and must not be substituted.

```sh
python3 deploy/gcp/manage.py --project example-finance-project seed \
  --instance YOUR_INSTANCE --zone us-west1-b --container EXISTING_CONTAINER \
  --env-file /private/path/.env.local-test > /private/path/secret-versions.json
```

The existing container's key takes precedence over the local file. Only the
three Wealthfolio credential assignments are imported; dotenv text is never
executed or interpolated. The CLI captures exports in memory and sends secret
values through stdin, never command arguments or deployment logs. The output
file contains only pinned secret **resource names**. Existing enabled versions
are reused only when their values match; differences stop deployment for an
explicit rotation decision.

If invitation authentication is not already configured, generate an invitation
with `scripts/create-owner-invitation.mjs` in a private directory outside Git.
Supply its `WF_SIGNUP_INVITE_HASH` assignment alongside the Resend key; keep
`owner-invitation.txt` private. The plaintext invitation is never uploaded to
GCP. Account mode replaces legacy password/OIDC login, so verify access to the
invitation before enabling it for an existing installation.

## Build the exact source

Commit the deployment changes and wait for relevant CI checks. Then:

```sh
python3 deploy/gcp/manage.py --project example-finance-project build --region us-west1
```

This creates/reuses a private `wealthfolio` Artifact Registry repository and
submits the clean committed source to Cloud Build. A dedicated builder has
repository-scoped Writer, source-bucket Viewer, and project Log Writer grants.
The operator also needs permission to act as that builder service account. The
output is an immutable image digest. Build inputs contain no runtime secrets:
both `.gcloudignore` and `.dockerignore` exclude local env files.

Cloud Build source archives are retained in the private staging bucket; apply
the project's source-artifact retention policy to that bucket. VM runtime
service accounts receive no access to it.

## Configure and install

Copy `config.example.json` to a private operator configuration file and replace
every placeholder using the inventory, build output, and secret-version output.
Pin the official Caddy image digest, even when reusing the current proxy.
`docker buildx imagetools inspect caddy:2-alpine` can retrieve its manifest
digest.

- Keep the existing data directory, database filename, and encryption
  requirement. Do not enable `require_db_encryption` on a plaintext database;
  conversion is a separate offline operation documented in
  `docs/self-host/README.md`.
- For a named Docker volume, set `data_volume` to its existing name as well as
  `data_dir` to the inspected mountpoint. It is reused as an external volume;
  Compose cannot silently create a replacement. Set `data_volume=null` for a
  host bind mount. Driver-managed/NFS volumes require a separate consistent
  backup plan and are rejected by this bootstrap.
- Set `manage_proxy=false` when an existing host proxy routes to the
  application's loopback port. A containerized existing proxy may need an
  explicit host/network route; do not assume its `localhost` is the VM's
  localhost.
- Set `manage_proxy=true` only when ports 80/443 are unused and the domain's A
  record points directly to this VM. This enables the bundled Caddy profile and
  adds a VM-specific firewall tag/rule for TCP 80/443. It does not open 8088.
- Reserve the existing external address before relying on it for permanent DNS.
- Confirm the selected data directory is readable/writable by image UID 1000.

```sh
python3 deploy/gcp/manage.py --project example-finance-project install \
  --instance YOUR_INSTANCE --zone us-west1-b --config /private/path/deployment.json
```

Installation grants the VM account Secret Accessor on exactly the selected three
secrets, plus Reader on the selected image repository. It does not grant project
Editor or Secret Accessor at project scope. Configuration and script metadata
contain resource references only. The VM fetches pinned versions with its own
identity, validates payload checksums, and verifies the master key matches the
currently running container **before stopping it**. Images are pulled and data
permissions checked before cutover.

The active nonsecret configuration is installed in
`/etc/wealthfolio/config.json` only after preflight and backup. Staging new VM
metadata cannot change the credentials used by the currently running service.

The selected app is stopped for a consistent pre-cutover data archive under
`/var/backups/wealthfolio` (root-only). Existing database contents are retained
in place. Systemd then supervises Compose; Docker does not independently restart
the app before its data and credentials are ready. A failed health check stops
the new service and preserves the backup. Initial HTTPS/auth verification
retries for up to three minutes to allow Caddy certificate provisioning;
exhaustion stops the service without creating release history. It does not
overwrite potentially migrated data automatically: stop the service and restore
the archive offline only after deciding how to handle any writes since cutover.

## Credential and access protections

- Master and Resend keys are fetched to `/run/wealthfolio`, a root-controlled
  memory-backed runtime directory, and bind-mounted read-only as UID 1000 files
  with mode `0400`. The server supports `WF_RESEND_API_KEY_FILE`; neither key is
  included in the container environment, image, or VM metadata.
- The temporary registry credential also stays under `/run`; it is not a
  downloaded service-account key. Versions are refreshed only by an explicit
  deployment/service restart, with numeric Secret Manager versions pinned.
- The app's host port binds to loopback. Caddy supplies HTTPS and the app
  requires secure cookies and an explicit origin. Container access to the IPv4
  metadata endpoint is blocked in `DOCKER-USER`; the host fetcher retains
  access. This package does not enable IPv6 Docker networking.
- Root/Docker administrators and a compromised application can still read the
  secrets needed by that application. Secret Manager is not a defense against
  full VM compromise.
- Existing env files, retired container metadata, VM snapshots, or old backups
  may contain legacy credentials. Audit and retire those copies after verifying
  the deployment and its recovery path; migration does not erase them silently.
- Preserve all installation data, including `web-account.json`, and its original
  master key. Keep recovery access to Secret Manager separate from VM backups.

Verify application health, HTTPS, signup/sign-in, recovery delivery, and reboot
persistence. Application health alone does not verify DNS/TLS or live Resend
delivery. Native credential integrations are unchanged.

## Local checks

```sh
python3 -m pip install -r deploy/gcp/requirements-test.txt
mkdir -p coverage/deployment
python3 -m coverage run --branch --source=deploy/gcp --omit='*/test_*.py' \
  --data-file=coverage/deployment/.coverage -m unittest discover -s deploy/gcp -p 'test_*.py' -v
python3 -m coverage json --data-file=coverage/deployment/.coverage -o coverage/deployment/coverage.json
python3 .github/scripts/deployment_coverage.py coverage/deployment/coverage.json
bash -n deploy/gcp/bootstrap.sh
sh -n deploy/gcp/block-metadata.sh
cargo test --locked -p wealthfolio-server account_auth --all-features
```

The deployment Python suite must reach at least 95% line coverage and 95% branch
coverage in combined totals. All deployment Python scripts stay in scope; only
test files are omitted. CI enforces both thresholds for PRs and production
release validation. Unit tests replace external cloud, Docker, and host-service
operations with fakes; actual image and VM rollout checks remain separate.

Architecture impact: adds deployment-only Secret Manager/IAM/build/VM
operations, a systemd lifecycle, and a read-once Resend key-file configuration
input. Existing environment input, account storage, encryption format, and
provider request behavior remain compatible. No background secret refresh or
application schema migration is introduced by this package.

## Restricted operator environments

Use the configured Google Cloud SDK normally. A managed secret can be a proxy
placeholder, so a hand-written bearer request is not an equivalent credential
test. When this environment supplies `GCP_ACCESS_TOKEN`, pass it to the SDK as
the process-only `CLOUDSDK_AUTH_ACCESS_TOKEN`; do not print it or write it into
a credential file. Set `CLOUDSDK_CONFIG` to a private writable directory. If the
home directory is read-only, set `WF_GCP_SSH_KEY_FILE` to a private writable SSH
key path for IAP/OS Login (an SSH key, not a service-account key).

Check credential readiness and network policy before running cloud commands. An
observed `ready` binding does not prove GCP accepts its value. A GCP `401`
requires a valid OAuth access token/identity; refresh `GCP_ACCESS_TOKEN` through
the environment's secret settings, never chat or Git. Do not disable TLS
verification or bypass the environment proxy.

In addition to Compute, Cloud Resource Manager, and IAM access, this workflow
uses these outbound destinations:

```text
secretmanager.googleapis.com
artifactregistry.googleapis.com
cloudbuild.googleapis.com
serviceusage.googleapis.com
storage.googleapis.com
iap.googleapis.com
oslogin.googleapis.com
tunnel.cloudproxy.app
us-west1-docker.pkg.dev
```

Allow the actual app hostname separately for the final HTTPS check. Credential
destination bindings should cover the intended Google API/IAP destinations;
never send the GCP bearer token to the app hostname. A proxy `403` requires a
supported network-policy update, not alternate routing.
