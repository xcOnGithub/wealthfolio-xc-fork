# Plaid connectivity test plan

Validate whether Plaid can supply the bank and investment data needed by our
self-hosted Wealthfolio installation before implementing an importer. Start with
Merrill Edge, then test the other six institutions. A successful login is only
the first check; the API must return usable accounts, balances, and history.

**Status:** Completed successfully in a separate private session, as reported by
the owner. Accepted as the feasibility baseline for the implementation plan.
Detailed evidence remains private; the worksheet below is the original template,
not a record of independently reproduced results.

**Country:** United States. Provider requirements below were reviewed on
November 3, 2026. Recheck the Dashboard's current eligibility and limits before
creating real connections.

## Scope and architecture impact

Run the official Plaid Quickstart on the account owner's computer in a separate
directory. First use Sandbox with fictitious accounts; use the Production API
under the Trial plan only after approval. Here, Production means Plaid's
real-account environment, not the production Wealthfolio installation.

This plan adds no Wealthfolio services, migrations, scheduled jobs, or data
writes. It leaves the GCP deployment, authentication, secrets, and existing
portfolio database unchanged. The test reads financial data through
owner-authorized Plaid connections. Creating and revoking those connections is
part of setup and cleanup; placing trades or moving money is excluded.

Device synchronization, cloud backups, Amex savings, advisor-managed Merrill
accounts, and a production importer are outside this test.

## Institution matrix

| Institution      | Accounts to select                             | Link product   | Required data                                         |
| ---------------- | ---------------------------------------------- | -------------- | ----------------------------------------------------- |
| Merrill Edge     | Owner-selected Merrill Edge brokerage accounts | `investments`  | Accounts, balances, holdings, investment transactions |
| Charles Schwab   | Owner-selected investment accounts             | `investments`  | Accounts, balances, holdings, investment transactions |
| Robinhood        | Owner-selected investment accounts             | `investments`  | Accounts, balances, holdings, investment transactions |
| Chase            | Owner-selected banking accounts                | `transactions` | Accounts, balances, posted transactions               |
| Bank of America  | Owner-selected banking accounts                | `transactions` | Accounts, balances, posted transactions               |
| U.S. Bank        | Owner-selected banking accounts                | `transactions` | Accounts, balances, posted transactions               |
| American Express | Credit cards only                              | `transactions` | Card accounts, balances, posted transactions          |

Confirm exact account types and the expected account count privately before
linking each institution. A bank name in an institution list does not establish
support for every account type or product. In particular, select the entry that
actually serves the Merrill Edge login, not an unrelated Merrill retirement or
benefits portal.

## Prerequisites and connection allowance

1. Create a [Plaid Dashboard account](https://dashboard.plaid.com/signup) and
   apply for the Trial plan if eligible. The owner completes Plaid's agreements,
   identity verification, and bank authorization themselves.
2. Confirm the Dashboard shows Trial access to Transactions and Investments.
   Trial currently includes both, along with Balance and other products.
3. Record the current Production Item count before the first live test.

An Item represents an institution-login connection, which may contain several
accounts. The Trial plan currently permits **10 Production Items**. Duplicate
Items count, and removing an Item does not restore its slot. Seven institutions
may fit if each requires one connection; separate household logins, existing
Items, or institution-specific linking behavior can change that calculation. Do
not upgrade to a paid plan merely to work around a failed test.

Use Sandbox for setup mistakes and practice. Reuse live Items and their access
tokens for subsequent reads instead of repeatedly linking the same login. See
[Plaid Trial requirements and limits](https://support.plaid.com/hc/en-us/articles/39994173227159-What-is-the-Plaid-Trial-plan).

## Credentials and private evidence

- Keep the Quickstart separate from the Wealthfolio checkout and production VM.
  Do not expose its demo UI or backend as a public service.
- Keep secrets in the local Quickstart `.env`, excluded from version control,
  with permissions restricted to the owner. Never place them in frontend code,
  screenshots, shared logs, chat, or this document.
- Preserve each returned access token in a password manager or encrypted local
  store before closing or restarting the demo. The Quickstart is a
  demonstration; do not rely on it to provide durable token storage or redacted
  output.
- Treat raw API responses, account identifiers, balances, and transactions as
  private financial data. Commit only sanitized test outcomes, not those values.
- Complete login and MFA through Plaid Link and the institution's authorization
  flow. Request only the products needed for this test and no payment or
  transfer capabilities. Keep TLS verification enabled.

## Phase 1 Set up the Quickstart in Sandbox

Install an active Node.js LTS version supported by the
[official Quickstart](https://github.com/plaid/quickstart), then run:

```bash
mkdir -p "$HOME/plaid-connectivity-test"
cd "$HOME/plaid-connectivity-test"
git clone https://github.com/plaid/quickstart.git
cd quickstart
umask 077
cp .env.example .env
chmod 600 .env
git check-ignore .env
```

The last command must confirm that `.env` is ignored before adding credentials.
Record the Quickstart revision with `git rev-parse HEAD` for reproducibility.
Edit `.env` locally:

```dotenv
PLAID_CLIENT_ID=REPLACE_WITH_CLIENT_ID
PLAID_SECRET=REPLACE_WITH_SANDBOX_SECRET
PLAID_ENV=sandbox
PLAID_PRODUCTS=investments
PLAID_COUNTRY_CODES=US
```

From the Quickstart directory, start the Node backend:

```bash
cd node
npm install
./start.sh
```

In a second terminal, from the Quickstart directory, start the frontend:

```bash
cd frontend
npm ci
npm start
```

Open the local URL printed by the frontend, normally `http://localhost:3000`.
Launch Link and use the fictitious credentials shown by Plaid. Verify account
listing, holdings, and investment transaction requests. Then practice the
banking flow with `PLAID_PRODUCTS=transactions`, restarting the backend after
editing configuration.

Sandbox passing establishes that the local setup works. It does not establish
real institution coverage or connectivity.

## Phase 2 Prepare the real account test

1. Confirm Trial approval and available Item slots.
2. Set the required Link use case and data-transparency configuration in the
   Dashboard.
3. Follow the Quickstart's
   [HTTPS localhost and OAuth instructions](https://github.com/plaid/quickstart#testing-oauth-with-a-redirect-uri-optional).
   Register the exact HTTPS callback URL in the Dashboard and configure
   `PLAID_REDIRECT_URI` accordingly. Do not use an HTTP callback for a
   Production redirect flow. Trial OAuth access does not replace callback
   configuration.
4. Replace the Sandbox secret with the Production secret locally and set:

   ```dotenv
   PLAID_ENV=production
   PLAID_PRODUCTS=investments
   PLAID_COUNTRY_CODES=US
   ```

5. Confirm how the access token will be preserved privately for repeat reads,
   then restart the backend and frontend as required by the HTTPS setup.

Keep only `investments` in the initial product list. Requiring unrelated
products such as `auth`, `signal`, and `transactions` together can exclude a
supported brokerage or request unnecessary access.

## Phase 3 Test Merrill Edge first

1. Launch Link, select the matching Merrill Edge institution entry, and
   authorize only the investment accounts the owner intends to track.
2. Exchange the returned public token once and preserve the resulting access
   token and Item ID privately. Record one new Item in the allowance worksheet.
3. Retrieve accounts, holdings, and investment transactions using the Quickstart
   requests or the API operations below. Start with a recent 90-day transaction
   window and follow pagination. If less history is available, record the actual
   range and whether it meets the owner's needs.
4. Compare account coverage and sample balances, position quantities, and known
   trades or dividends with Merrill's own website or statements. Account for
   differences in valuation timestamps and market-price updates.
5. Repeat the API reads using the same access token. Do not launch a new Link
   session just to read the same account again.

The first investment-transactions request can take one or two minutes while
initial data is retrieved. A connected account with unavailable or incomplete
holdings/history is a partial result, not a complete pass. No transaction
records are expected for an account that genuinely had no activity during the
requested period. See [Plaid Investments](https://plaid.com/docs/investments/).

If the exact Merrill Edge entry is unavailable, cannot authorize, or cannot
provide the required investment data, record the error and classify the result
before spending another Item slot. Resolve the Merrill gap before committing to
the full importer; success at other institutions does not resolve it.

## Phase 4 Test the other six institutions

For Schwab and Robinhood, repeat the investment checks with
`PLAID_PRODUCTS=investments`.

For Chase, Bank of America, U.S. Bank, and Amex credit cards, change the product
list to `transactions` and restart the backend before creating new connections.
Do not relink Merrill Edge when changing the configuration for a different
institution. Keep each existing Item's token available for its own API reads.

For each banking connection, confirm account types, current and available
balances where supplied, posted transaction coverage, dates, currency, and
descriptions. Treat missing or null values as missing data rather than zero.
Amex savings must remain unselected.

| API operation                   | Purpose                                            | Validation                                                                  |
| ------------------------------- | -------------------------------------------------- | --------------------------------------------------------------------------- |
| `/accounts/get`                 | Read account metadata and available balance fields | Expected accounts and types; do not assume cached balances are real-time    |
| `/investments/holdings/get`     | Read securities and positions                      | Quantities, currencies, identifiers, and cost basis where provided          |
| `/investments/transactions/get` | Read investment history                            | Trades, dividends, fees, transfers, dates, and pagination                   |
| `/transactions/sync`            | Read bank and card transaction changes             | Follow cursors and `has_more`; inspect added, modified, and removed records |
| `/item/get`                     | Inspect the existing connection                    | Institution, product availability, and Item errors                          |

These are Plaid API operation paths, not new Wealthfolio endpoints. The
Quickstart or an isolated API client supplies the credentials privately. A
dedicated connectivity probe has not yet been implemented in this repository.
For multi-Item repeat testing, the client must load the saved token for each
existing Item instead of creating another connection.

Do not treat bank and investment transaction schemas as interchangeable.
Investment transaction amounts use their own sign conventions. Pending bank
transactions can be replaced by posted records with different IDs; unchanged
posted records should retain stable identifiers. Record these behaviors for the
later importer rather than writing financial records during this test.

## Phase 5 Check freshness and reconnection behavior

Read the same Items again on the next business day, then once per day for three
additional business days. Record provider timestamps, new data, errors, and any
required owner reauthorization. Avoid rapid refresh loops and do not create real
transactions or trades merely to generate test data.

Investments are generally refreshed at least once per market day, with timing
depending on the institution. Evaluate freshness against that schedule and known
institution activity, not an assumption of real-time stock prices. If the test
period has no source changes, classify automatic-update verification as
inconclusive rather than claiming it passed. Reuse saved access tokens and
request only relevant history or transaction changes on repeat reads.

## Acceptance criteria

| Check             | Pass condition                                                                                                                                           |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Account coverage  | Every owner-selected account appears under the correct institution and account type                                                                      |
| Required data     | Banking connections return balances and posted transactions; brokerage connections return balances, holdings, and investment history needed by the owner |
| Accuracy          | Privately checked samples agree with the institution after accounting for timestamps and documented field limitations                                    |
| History           | Pagination is complete for the available requested range, and any shorter range is explicitly accepted                                                   |
| Repeated reads    | The existing Item can be read again; unchanged posted records have stable identifiers and changes are distinguishable                                    |
| Freshness         | Known source changes appear within the provider's documented refresh behavior                                                                            |
| Unattended access | Reads work without a new interactive login on every request; required reconnections are recorded                                                         |
| Data protection   | No credentials or raw financial responses enter GitHub, shared logs, or the production Wealthfolio database                                              |

Use **Pass**, **Partial**, **Fail**, **Blocked**, or **Not run**. Partial means
the connection works but required data or history is missing. Blocked means a
setup, eligibility, permission, or access issue prevents evaluating
connectivity. Do not label an institution unsupported solely because a local
callback or product configuration is wrong.

## Results worksheet

| Institution       | Link    | API data | Repeat and freshness | Overall | Sanitized notes |
| ----------------- | ------- | -------- | -------------------- | ------- | --------------- |
| Merrill Edge      | Not run | Not run  | Not run              | Not run |                 |
| Charles Schwab    | Not run | Not run  | Not run              | Not run |                 |
| Robinhood         | Not run | Not run  | Not run              | Not run |                 |
| Chase             | Not run | Not run  | Not run              | Not run |                 |
| Bank of America   | Not run | Not run  | Not run              | Not run |                 |
| U.S. Bank         | Not run | Not run  | Not run              | Not run |                 |
| Amex credit cards | Not run | Not run  | Not run              | Not run |                 |

For each institution, record the test date and timezone, Quickstart revision,
products requested, expected/returned account counts, available history range,
data freshness, provider error codes, and reconnection count. Keep actual
account identifiers, balances, transaction values, access tokens, and detailed
comparison evidence in private storage outside the repository.

| Trial allowance                                  | Count     |
| ------------------------------------------------ | --------- |
| Production Items already consumed before testing | To record |
| New Items consumed during this test              | To record |
| Remaining slots under the current Trial limit    | To record |

## Decision and cleanup

Proceed to an isolated Wealthfolio importer only when the required data passes
for the institutions it will support. Explicitly agree on any partial coverage
before implementation. Evaluate SnapTrade Personal for unresolved brokerages and
SimpleFIN or another bank-feed provider for unresolved banks, with their own
coverage, permissions, and cost checks.

Before any production import, separately test deduplication, pending-to-posted
changes, corrections and removals, transfer handling, investment mappings, and
preservation of manual edits in a disposable Wealthfolio database. Connectivity
passing alone does not verify those behaviors.

At the end, stop the Quickstart services. Revoke unneeded connections through
Plaid and the institution, invalidate the corresponding tokens, and remove
unneeded local secrets and private exports. Remember that removing Trial Items
does not restore their allowance. Retain a connection only if the owner wants
continued testing and its token remains securely stored.

## References

- [Plaid Trial plan and limits](https://support.plaid.com/hc/en-us/articles/39994173227159-What-is-the-Plaid-Trial-plan)
- [Plaid Quickstart guide](https://plaid.com/docs/quickstart/)
- [Official Quickstart repository and OAuth setup](https://github.com/plaid/quickstart)
- [Quickstart environment variables](https://github.com/plaid/quickstart/blob/master/.env.example)
- [Plaid Investments](https://plaid.com/docs/investments/)
- [Plaid Transactions](https://plaid.com/docs/transactions/)
