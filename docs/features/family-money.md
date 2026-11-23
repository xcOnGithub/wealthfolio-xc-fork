# Family inbox, alerts, and money meeting

This feature is scoped to an existing financial profile. It does not add
household accounts or change owner authentication. A partner can receive a
configured digest without signing into the app. Email and webhooks are off until
explicitly enabled.

## Architecture impact

A shared `wealthfolio-family` service owns detection, inbox lifecycle, digest
composition, and delivery decisions. It reads existing spending, budget, goal,
net-worth, drift, account, snapshot, and locally stored broker state. Web and
Tauri use the same service; the inbox never initiates a broker or market-data
sync. Unknown valuations/FX stay unavailable, rather than becoming zero.

One new profile-local SQLite state row persists notification metadata, inbox
acknowledgements/snoozes, monthly digests, and delivery claims. Existing
preferences are device-synced; putting delivery claims there could cause another
installation to send duplicates. Existing broker staging tables cannot represent
unrelated family tasks. The new row therefore has no device-sync outbox and uses
the existing SQLite writer and database encryption policy. Secrets use
SecretStore only. Inbox decisions and delivery configuration are local to this
profile installation; device sync does not copy them. Configure delivery on one
always-running installation to avoid independent notifications from multiple
devices.

The existing server background-worker lifecycle and Tauri runtime worker
lifecycle schedule evaluation. Each runtime spawns the shared worker loop with
its own task API (Tokio for the server, Tauri for desktop/mobile) and retains
the abort handle in its existing worker list. Offline/sleeping apps catch up for
the last complete calendar month. A desktop app must be running; the web server
can run unattended. Monthly digests become due at 09:00 profile-local time on
the first day of the month.

## Behavior

- One inbox contains uncategorized/flagged cash transactions, other flagged
  activities, pending broker/statement review, unhealthy connections, stale
  manual prices/holdings snapshots, and proactive alerts.
- Acknowledge, snooze, resolve, reopen, and leave a family note. A failed source
  read must not auto-resolve its existing items. Detection updates facts without
  overwriting these user decisions. A cleared condition that later recurs opens
  a new episode; a persistent condition does not notify on every tick.
- Configurable large-charge and unusual-merchant thresholds, budget percentage,
  consent notice window, stale valuation age, and manually scheduled annual
  bills. Unusual charges require a sufficient same-merchant history in one
  currency. Charge alerts consider the last seven days and compare up to 90 days
  of earlier merchant history.
- Allocation alerts use existing target-band calculations and scope resolution.
- Digest: prior calendar month's net-worth change, savings rate (income less
  consumption, divided by income), category spending movers versus the prior
  calendar month, stored goal progress, and next month's configured big bills.
  No AI upload is needed. Partial data is explicitly marked unavailable.
- Consent alerts use an actual provider consent-expiration timestamp; a Plaid
  Link token expiry is never treated as consent expiry.
- Invalid channel configuration is rejected against the proposed credentials
  before any credential writes.
- Notifications are batched. Delivery settings apply to future queued messages;
  changing recipients cancels old pending deliveries instead of retargeting
  them. Claims are persisted before network I/O. Failed/uncertain deliveries are
  visible and require explicit retry, preventing silent duplicate webhook sends
  after a crash. Resend and webhooks receive a stable idempotency key.
- Webhooks require public HTTPS destinations, pinned DNS resolution, no
  redirects, bounded requests, and no credentials in URLs. Test transports never
  send real mail.

Statement previews register an account-scoped reminder; a successful save clears
it. The inbox keeps no document contents, so returning to a preview requires
selecting the original files again. Reminder failures do not block imports.

The initial generated alert descriptions and digest prose are English. The UI
uses the existing localization namespace with English copy in each catalog.

## Validation

Tests cover source completeness, unknown versus zero accounting,
calendar/timezone boundaries, recurrence/snooze/deduplication, profile-local
storage, delivery opt-in, secret redaction, failures, and both runtime adapters.
Browser checks exercise the inbox/settings/digest flow in LTR and RTL with
synthetic data.
