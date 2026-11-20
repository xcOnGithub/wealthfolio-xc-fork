# Family Finance Center Portal — Feature Roadmap & Backlog

Reference backlog of candidate features to evolve this Wealthfolio fork from an
individual portfolio and spending tracker into a shared **Family Finance Center
Portal**.

## Current Baseline

Wealthfolio (plus this fork's additions) already provides a strong foundation:

- **Investments & Net Worth:** Multi-account tracking (`SECURITIES`, `CASH`,
  `CREDIT_CARD`, `CRYPTOCURRENCY`), liabilities and loans with amortization
  schedules, alternative assets (real estate, vehicles, collectibles),
  multi-currency FX, cost-basis lots (`FIFO`, `LIFO`, `HIFO`, `WAC`),
  rebalancing targets, TWR/MWR performance, dividend/interest history, and
  portfolio health checks.
- **Spending & Planning:** Categorization rules, transaction splits, budgets,
  save-up simulations, retirement/FIRE planning, and recurring contribution
  limits.
- **Fork Additions & Platform:** Direct Plaid integration (`DIRECT_PLAID`),
  monthly statement history and Merrill statement reconciliation, single-owner
  web auth with email recovery (`apps/server/src/account_auth.rs`), OIDC SSO,
  encrypted SQLite + secret storage, and AI assistant / MCP access.

---

## Tier 1: Core "Family Portal" Foundations

### 1. Household Members, Ownership Splits & Multi-User Roles

- **Current gap:**
  - `crates/core/src/profiles/mod.rs` isolates each profile in a separate SQLite
    database (`profiles/<uuid>/app.db`), which prevents unified household views
    without duplicating data.
  - `apps/server/src/account_auth.rs` is explicitly single-owner without tenant
    or role separation.
  - `Account` (`crates/core/src/accounts/accounts_model.rs`) only has a freeform
    `group` string and `crates/core/src/portfolios/` groups accounts without
    modeling people or ownership percentages.
- **Proposed capabilities:**
  - First-class **Household Members** (e.g., Partner A, Partner B, Children) and
    **Account/Asset/Liability Ownership** (`Individual`, `Joint 50/50`, or
    custom split %) inside a single shared household database.
  - Global view switcher (_Whole Family_, _Partner A_, _Partner B_, _Kids_)
    applied consistently across Net Worth, Holdings, Spending, Contribution
    Limits, and Goals.
  - **Multi-user web portal logins & roles** (e.g., Partner/Admin with full
    access, Spending-only/Editor, Read-only Family Viewer, or time-limited
    CPA/Advisor read-only access) layered onto existing web auth / OIDC.

### 2. Recurring Bills, Subscriptions & Forward Cash-Flow Calendar

- **Current gap:**
  - Spending (`apps/frontend/src/features/spending/pages/`) tracks historical
    outflows and budgets, and loans track amortization, but there is no
    forward-looking bill manager or short-term liquidity forecast.
- **Proposed capabilities:**
  - **Recurring Bills & Subscriptions Registry** covering mortgage/rent,
    childcare, utilities, property taxes, and annual memberships—auto-suggested
    from Plaid/card transaction patterns or entered manually—with renewal
    reminders and price-increase detection.
  - **30/60/90-Day Household Cash-Flow Forecast** combining expected paychecks,
    scheduled bills, credit card statement due dates, and estimated tax payments
    against checking balances.
  - **Emergency Fund Runway Gauge** separating liquid cash/money-market/T-bill
    balances from illiquid/invested assets and measuring months of _actual_
    essential household spending covered.

### 3. Alerts, Unified "Needs Attention" Inbox & Monthly Family Digest

- **Current gap:**
  - `apps/server/src/scheduler.rs` currently only runs periodic broker syncs.
  - Resend email delivery is already wired in `apps/server/src/account_auth.rs`,
    but there is no user notification or digest subsystem.
- **Proposed capabilities:**
  - **Unified "Needs Attention" Inbox** consolidating uncategorized or flagged
    transactions, pending Plaid/statement reconciliations, broken institution
    consents, and stale manual asset valuations (home, vehicles, manual 401(k)
    balances).
  - **Proactive Alerts** (in-app + optional email/webhook): institution sync
    failures or expiring consents, large or unusual card charges, approaching
    budget limits, upcoming annual bills/renewals, low checking balance, and
    allocation drift outside configured bands.
  - **Automated Monthly "Family Money Meeting" Digest** (email and printable
    portal view): Net Worth delta, Savings Rate, top spending changes, goal
    progress, and upcoming major expenses—tailored for a spouse who prefers a
    high-level monthly check-in.

---

## Tier 2: Household Tax, Income & Collaboration

### 4. Family Tax Center & Cross-Account Awareness

- **Current gap:**
  - Lots, disposals, trade taxes, contribution limits
    (`crates/core/src/limits/limits_model.rs`), and statement history exist as
    separate tools; `ROADMAP.md` still lists cross-account cost basis and
    specific-lot selection as upcoming.
- **Proposed capabilities:**
  - **YTD Household Tax Dashboard:** Short-term vs. long-term realized capital
    gains, qualified vs. ordinary dividends, interest income, foreign tax
    withheld, and estimated tax liability.
  - **Cross-Account Wash-Sale & Tax-Loss Harvesting Scanner:** Detect wash-sale
    risk across _all_ family accounts (including across spouses' taxable
    brokerage and IRA accounts) and surface unrealized harvesting opportunities.
  - **Per-Member Tax-Advantaged Limits:** Associate `ContributionLimit` records
    with specific members (Partner A 401(k)/IRA/Backdoor Roth, Partner B
    401(k)/IRA, Family HSA, FSA use-it-or-lose-it deadlines, Kids' 529s) with
    Dec 31 vs. Tax Day countdowns.
  - **Asset Location View:** Highlight tax-inefficient holdings sitting in
    taxable vs. tax-deferred vs. tax-free buckets.
  - **One-Click Year-End Tax Packet Export:** Bundle Form 8949-style realized
    gains CSV, dividend/interest summaries, charitable giving totals, and linked
    statements for CPA/tax-filing prep.

### 5. Full Household Income, Paycheck & Equity Comp Tracking

- **Current gap:**
  - `apps/frontend/src/pages/income/income-page.tsx` tracks _investment_ income
    (dividends and interest), not household earned income.
- **Proposed capabilities:**
  - **Gross-to-Net Paycheck Modeling:** Track salaries, bonuses, employer
    retirement matches, taxes withheld, and pre-tax deductions (health/HSA/401k)
    alongside investment and rental income.
  - **True Household Savings Rate & Sankey Cash-Flow View:** Visualize Gross
    Income -> Taxes -> Fixed Needs -> Discretionary Wants -> Debt Paydown ->
    Savings/Investments.
  - **Equity Compensation Tracker (RSU / ESPP / Options):** Vesting calendar,
    estimated tax-withholding shortfall alerts, and single-stock concentration
    monitoring against total household net worth.

### 6. Lightweight Partner Collaboration

- **Current gap:**
  - Transactions support categories and splits, but there is no shared review or
    household settlement workflow.
- **Proposed capabilities:**
  - **Transaction Notes, Mentions & Review Flags** (e.g., flag a charge for a
    partner to clarify or attach context).
  - **Shared Expense Split & Monthly True-Up** for households that maintain a
    mix of individual accounts and joint accounts and settle shared expenses
    periodically.

---

## Tier 3: Protection, Estate Readiness & Life Planning

### 7. Encrypted Family Document Vault & Emergency ("ICE") Binder

- **Current gap:**
  - `apps/frontend/src/features/statement-history/` handles monthly brokerage
    statements, but broader family financial records have no structured home.
- **Proposed capabilities:**
  - **Local Encrypted Document Vault** attaching PDFs and receipts to accounts,
    properties, loans, or tax years (1040s, W-2/1099s, term life/umbrella/home/
    auto insurance policies with renewal tracking, property deeds, wills/trusts,
    major warranties).
  - **Beneficiary & Account Directory ("Emergency Binder"):** Structured
    checklist per account tracking institution, account type, primary/contingent
    beneficiaries, POD/TOD status, autopay funding account, and last-verified
    date—exportable as an encrypted or printable family reference binder.

### 8. Multi-Milestone Family Planning & Shock Stress-Testing

- **Current gap:**
  - `crates/core/src/planning/` supports standalone save-up goals and retirement
    (FIRE) simulations, without layered household life events.
- **Proposed capabilities:**
  - **Combined Household Milestone Timeline:** Layer children's education (529
    projections per child), home purchase/refinance/payoff-vs-invest trade-offs,
    and two-spouse retirement timelines (different ages, Social Security/pension
    start dates, pre-Medicare healthcare bridge).
  - **"What-If" Household Stress Tests:** Model temporary single-income periods
    (parental leave, sabbatical, job loss), daycare start/end cliffs, or major
    home repairs.

---

## Suggested Implementation Phases

1. **Phase 1 — Household Members & Ownership Tagging:** Adds family-wide vs.
   per-member filtering across existing pages without splitting the database.
2. **Phase 2 — Unified Inbox, Alerts & Monthly Digest:** Reuses existing
   background scheduling and Resend email integration to make the portal
   proactive for the whole household.
3. **Phase 3 — Recurring Bills & 90-Day Cash-Flow Runway:** High day-to-day
   utility paired with existing Plaid credit card and bank feeds.
4. **Phase 4 — Family Tax Center & Per-Member Contribution Limits:** Builds
   directly on existing lots, disposals, limits, and statement-history records.
5. **Phase 5 — Household Income/Savings Rate, Document Vault & Multi-Milestone
   Planning:** Rounds out long-term family planning and estate readiness.

---

## Architectural & Privacy Guardrails to Preserve

- Keep **local-first SQLite** as the single source of truth; avoid introducing
  external cloud dependencies for financial data or documents.
- Route any new credentials or document encryption keys through `SecretStore`
  (`crates/core/src/secrets/mod.rs`) and never log financial rows or secrets.
- Preserve existing opt-in sync boundaries (`DIRECT_PLAID`, Wealthfolio Connect)
  and keep notifications/digests opt-in and configurable.
- Follow existing runtime parity rules (shared business logic in `crates/core/`,
  thin Tauri commands / Axum handlers, logical RTL styling on all new UI).
