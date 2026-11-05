# Direct Plaid on the existing VM

Merge and deploy the reviewed image through the existing CI/CD pipeline first.
Plaid stays off until the installation configuration explicitly enables it.
GitHub Actions builds the image without Plaid credentials. The VM service
account reads a pinned Secret Manager version at service start; the container
receives a read-only credential file. Individual Item/access/Link tokens use the
existing profile-scoped encrypted SecretStore, and financial staging uses the
encrypted profile database. Preserve the installation master key and entire data
directory in backups; a database restore without matching profile credentials is
disconnected.

## Prepare privately

1. In Plaid, enable the required Transactions/Investments products and Hosted
   Link/OAuth support. Register the application's exact HTTPS return URI:
   `https://YOUR_DOMAIN/settings/accounts`. Production access,
   institution/product availability, consent expiration and Plaid charges depend
   on your Plaid account.
2. Use the institution IDs verified in the owner's private connectivity tests.
   Supported configuration keys are `chase`, `boa`, `usbank`, `amex`,
   `merrill_edge`, `schwab`, and `robinhood`. Amex permits credit cards only;
   Merrill Edge requires confirmation that the account is self-directed.
3. Store the Plaid environment secret in a private local file with mode `0600`.
   Do not put it in repository configuration, command-line arguments, GitHub
   variables, chat, or CI output. Use the existing authenticated operator
   identity:

   ```bash
   gcloud secrets create wealthfolio-plaid-secret --project=example-finance-project --replication-policy=automatic
   gcloud secrets versions add wealthfolio-plaid-secret --project=example-finance-project --data-file=/PRIVATE/PATH/plaid-secret
   ```

   If the secret already exists, add a version only. Record the numeric version
   identifier; never configure `latest`. Remove the temporary private file once
   it is safely stored. Sandbox and production credentials must stay separate.

## Configure and activate

Start from the current inspected installation plan. Preserve its image digest,
existing data mount, master key, invitation hash, email configuration, and proxy
settings. Add the following non-secret fields, replacing placeholders:

```json
{
  "require_db_encryption": true,
  "plaid": {
    "enabled": true,
    "environment": "sandbox",
    "client_id": "YOUR_PLAID_CLIENT_ID",
    "redirect_uri": "https://YOUR_DOMAIN/settings/accounts",
    "institutions": { "chase": "ins_VERIFIED_NUMERIC_ID" }
  },
  "secrets": {
    "plaid_secret": "projects/example-finance-project/secrets/wealthfolio-plaid-secret/versions/NUMERIC_VERSION"
  }
}
```

These are additions to the installation configuration, not a replacement file.
Keep all three existing secret references. The institution and version examples
are placeholders and fail validation until replaced. Encrypted database mode is
required: do not flip an existing plaintext installation's setting without using
its supported encryption migration and verifying a backup first.

Use the existing `manage.py` installation-plan inspection/installation flow in
[README.md](README.md) and [CICD.md](CICD.md) for VM `YOUR_INSTANCE`, zone
`us-west1-b`. Review the concrete plan before activating it. The installer
grants the VM service account accessor permission on the optional Plaid secret
only when enabled. Reinstall the reviewed deployment scripts/configuration on an
existing VM; an ordinary image-only release does not update those host scripts.

At service start `fetch-secrets.py` validates all settings and fetched payloads
before replacing credential files. It creates root-owned runtime directories and
a UID 1000, mode `0400` `plaid-secret` file, mounted read-only at
`/run/secrets/plaid-secret`. Containers cannot fetch the VM identity directly;
the existing metadata firewall rule remains required. Allow outbound HTTPS to
`sandbox.plaid.com` or `production.plaid.com`, and browser access to Hosted
Link.

## Connect and verify

Sign in and unlock the intended profile, open Settings → Accounts, and use the
direct Plaid panel. Link one institution first. Map each selected provider
account to a compatible local account; review opening balances or choose a
non-overlapping bank cutover date. Explicitly classify ambiguous transfers and
approve both legs when pairing them. For brokerages, preview holdings and
explicitly approve opaque securities when necessary. Investment source
transactions are view-only and do not rebuild historical returns. Confirm
balances, credit-card signs, positions, cash, latest provider price dates and
next daily sync before expanding rollout.

Trials may count lifetime Items rather than active connections. Deleting an Item
does not necessarily recover quota. To reuse a privately tested Item, the server
has an owner-only `adopt` operation at `POST /api/v1/plaid/action` with a JSON
body `{"operation":"adopt","institution":"chase","accessToken":"…"}`. Supply
this only from a private operator process using the authenticated session,
current profile admission headers, exact application Origin, and request bytes
from a protected file or standard input. Never use shell arguments containing
the access token. Adoption verifies the Item/institution/product before storing
its token and never revokes the pre-existing Item on validation failure. There
is deliberately no browser token-input form or generic secret export endpoint.

## Disable, rotate, or recover

- To pause the feature, set `plaid.enabled` to `false` and activate the reviewed
  configuration. Startup makes zero Plaid requests, does not read the optional
  secret, removes the stale runtime secret file, and stops its scheduler. The
  optional pinned reference can remain in configuration. Local history remains.
- Disablement is not consent revocation. Disconnect each Item in the UI while
  enabled, or revoke it in Plaid's Dashboard. A failed revocation stays pending
  and can be retried; it is never reported as completed prematurely.
- For secret rotation, add a new Secret Manager version and pin it in the
  reviewed installation plan, then restart. Do not rotate the installation
  master key as part of Plaid credential rotation.
- An ambiguous single-use exchange is not retried. An access token saved before
  an interrupted SQLite write resumes validation on the next authenticated
  Finish. If no token was saved, inspect/revoke the Item in Plaid before
  creating a replacement. If both local token persistence and remote cleanup
  fail, use Dashboard revocation.
- Provider deletions involving edited/grouped local activities pause that
  mapping for owner review. Resolve deliberately through the activity editor or
  keep it paused; the importer never silently discards owner edits.
- Roll back with Plaid disabled to a reviewed compatible image. The migration
  adds local tables without deleting financial rows; preserve them and the
  credential store. A compatible rollback image must retain `DIRECT_PLAID`
  exclusion from hosted Connect jobs. Do not run the destructive down migration
  on live data.
