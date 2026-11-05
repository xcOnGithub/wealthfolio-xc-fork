use std::{fmt, str::FromStr, time::Duration};

use chrono::{NaiveDate, Utc};
use reqwest::{
    header::{HeaderMap, HeaderValue},
    redirect::Policy,
    Url,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::models::*;

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaidEnvironment {
    Sandbox,
    Production,
}

impl PlaidEnvironment {
    fn base_url(self) -> &'static str {
        match self {
            Self::Sandbox => "https://sandbox.plaid.com",
            Self::Production => "https://production.plaid.com",
        }
    }
}

impl FromStr for PlaidEnvironment {
    type Err = PlaidError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "sandbox" => Ok(Self::Sandbox),
            "production" => Ok(Self::Production),
            _ => Err(PlaidError::Configuration),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaidProduct {
    Transactions,
    Investments,
}

/// A token bound to its provider environment. Never serialized or logged.
pub enum Access {}
pub enum Public {}
pub enum Link {}
pub type AccessToken = PlaidToken<Access>;
pub type PublicToken = PlaidToken<Public>;
pub type LinkCredential = PlaidToken<Link>;

pub struct PlaidToken<Kind> {
    kind: std::marker::PhantomData<Kind>,
    environment: PlaidEnvironment,
    value: Zeroizing<String>,
}

impl<Kind> PlaidToken<Kind> {
    pub fn new(environment: PlaidEnvironment, value: String) -> Result<Self, PlaidError> {
        let value = Zeroizing::new(value);
        validate_opaque(&value)?;
        Ok(Self {
            environment,
            value,
            kind: std::marker::PhantomData,
        })
    }

    /// Only for encrypted SecretStore persistence or an authorized Link response.
    pub fn expose_secret(&self) -> &str {
        &self.value
    }
}

impl<Kind> fmt::Debug for PlaidToken<Kind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlaidToken([REDACTED])")
    }
}

/// Configuration is constructed explicitly: no environment lookup at module load.
pub struct PlaidConfig {
    environment: PlaidEnvironment,
    headers: HeaderMap,
    redirect_uri: Url,
}

impl fmt::Debug for PlaidConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlaidConfig([REDACTED])")
    }
}

impl PlaidConfig {
    /// Disabled configuration is inert, even if unused values are invalid.
    pub fn when_enabled(
        enabled: bool,
        environment: &str,
        client_id: &str,
        secret: &str,
        redirect_uri: &str,
    ) -> Result<Option<Self>, PlaidError> {
        if !enabled {
            return Ok(None);
        }
        let environment = environment.parse()?;
        let redirect_uri = Url::parse(redirect_uri).map_err(|_| PlaidError::Configuration)?;
        if redirect_uri.scheme() != "https"
            || !redirect_uri.username().is_empty()
            || redirect_uri.password().is_some()
            || redirect_uri.query().is_some()
            || redirect_uri.fragment().is_some()
        {
            return Err(PlaidError::Configuration);
        }
        let mut headers = HeaderMap::new();
        for (name, value) in [("plaid-client-id", client_id), ("plaid-secret", secret)] {
            validate_opaque(value)?;
            let mut header = HeaderValue::from_str(value).map_err(|_| PlaidError::Configuration)?;
            header.set_sensitive(true);
            headers.insert(name, header);
        }
        headers.insert("plaid-version", HeaderValue::from_static("2020-09-14"));
        Ok(Some(Self {
            environment,
            headers,
            redirect_uri,
        }))
    }
}

/// Deliberately contains no provider response text, URLs, or financial payloads.
#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum PlaidError {
    #[error("Plaid configuration or request is invalid")]
    Configuration,
    #[error("Plaid credentials are invalid")]
    Credentials,
    #[error("Plaid connection requires attention")]
    ReconnectRequired,
    #[error("Plaid rate limit reached")]
    RateLimited,
    #[error("Plaid data is not ready")]
    NotReady,
    #[error("Plaid pagination changed; restart the complete batch")]
    PaginationChanged,
    #[error("Plaid is unavailable")]
    Unavailable,
    #[error("Plaid returned an invalid response")]
    InvalidResponse,
    #[error("Plaid rejected the request")]
    Rejected,
}

pub struct PlaidClient {
    http: reqwest::Client,
    config: PlaidConfig,
    base_url: String,
}

impl fmt::Debug for PlaidClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PlaidClient([REDACTED])")
    }
}

fn validate_opaque(value: &str) -> Result<(), PlaidError> {
    if value.is_empty()
        || value.len() > 16384
        || value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(PlaidError::Configuration);
    }
    Ok(())
}

#[derive(Deserialize)]
struct ErrorBody {
    error_code: String,
}

fn classify_error(status: u16, body: &[u8]) -> PlaidError {
    if status == 429 {
        return PlaidError::RateLimited;
    }
    if status >= 500 {
        return PlaidError::Unavailable;
    }
    let code = serde_json::from_slice::<ErrorBody>(body)
        .map(|b| b.error_code)
        .unwrap_or_default();
    match code.as_str() {
        "INVALID_API_KEYS" | "INVALID_ACCESS_TOKEN" => PlaidError::Credentials,
        "ITEM_LOGIN_REQUIRED"
        | "ITEM_LOCKED"
        | "USER_PERMISSION_REVOKED"
        | "ITEM_NOT_SUPPORTED" => PlaidError::ReconnectRequired,
        "PRODUCT_NOT_READY" | "NO_ACCOUNTS" => PlaidError::NotReady,
        "TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION" => PlaidError::PaginationChanged,
        "RATE_LIMIT_EXCEEDED" => PlaidError::RateLimited,
        _ => PlaidError::Rejected,
    }
}

impl PlaidClient {
    pub fn new(config: PlaidConfig) -> Result<Self, PlaidError> {
        let http = wealthfolio_http::client_builder()
            .timeout(TIMEOUT)
            .retry(reqwest::retry::never())
            .redirect(Policy::none())
            .build()
            .map_err(|_| PlaidError::Configuration)?;
        let base_url = config.environment.base_url().to_string();
        Ok(Self {
            http,
            config,
            base_url,
        })
    }

    fn token<'a, Kind>(&self, token: &'a PlaidToken<Kind>) -> Result<&'a str, PlaidError> {
        if token.environment != self.config.environment {
            return Err(PlaidError::Configuration);
        }
        Ok(token.expose_secret())
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: Value) -> Result<T, PlaidError> {
        let mut response = self
            .http
            .post(format!("{}{path}", self.base_url))
            .headers(self.config.headers.clone())
            .json(&body)
            .send()
            .await
            .map_err(|_| PlaidError::Unavailable)?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| PlaidError::Unavailable)?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(PlaidError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        if !(200..300).contains(&status) {
            return Err(classify_error(status, &bytes));
        }
        serde_json::from_slice(&bytes).map_err(|_| PlaidError::InvalidResponse)
    }

    pub async fn create_link_token(
        &self,
        client_user_id: &str,
        product: PlaidProduct,
    ) -> Result<LinkToken, PlaidError> {
        validate_opaque(client_user_id)?;
        self.link(
            json!({"client_name":"Wealthfolio", "language":"en", "country_codes":["US"],
            "user":{"client_user_id":client_user_id}, "products":[product],
            "redirect_uri":self.config.redirect_uri.as_str()}),
        )
        .await
    }

    /// Update mode repairs an existing Item; it does not create a second Item.
    pub async fn create_update_link_token(
        &self,
        client_user_id: &str,
        token: &AccessToken,
    ) -> Result<LinkToken, PlaidError> {
        validate_opaque(client_user_id)?;
        self.link(
            json!({"client_name":"Wealthfolio", "language":"en", "country_codes":["US"],
            "user":{"client_user_id":client_user_id}, "access_token":self.token(token)?,
            "redirect_uri":self.config.redirect_uri.as_str()}),
        )
        .await
    }

    async fn link(&self, body: Value) -> Result<LinkToken, PlaidError> {
        #[derive(Deserialize)]
        struct Response {
            link_token: String,
            expiration: chrono::DateTime<Utc>,
        }
        let response: Response = self.post("/link/token/create", body).await?;
        let link_token = PlaidToken::new(self.config.environment, response.link_token)
            .map_err(|_| PlaidError::InvalidResponse)?;
        Ok(LinkToken {
            link_token,
            expiration: response.expiration,
        })
    }

    /// Session ownership/association must be verified by the caller before exchange.
    /// Never automatically retry this single-use operation after an ambiguous failure.
    pub async fn exchange_public_token(
        &self,
        token: &PublicToken,
    ) -> Result<TokenExchange, PlaidError> {
        #[derive(Deserialize)]
        struct Response {
            item_id: String,
            access_token: String,
        }
        let response: Response = self
            .post(
                "/item/public_token/exchange",
                json!({"public_token":self.token(token)?}),
            )
            .await?;
        validate_opaque(&response.item_id).map_err(|_| PlaidError::InvalidResponse)?;
        let access_token = PlaidToken::new(self.config.environment, response.access_token)
            .map_err(|_| PlaidError::InvalidResponse)?;
        Ok(TokenExchange {
            item_id: response.item_id,
            access_token,
        })
    }

    /// Hosted Link provides server-verifiable results without Link-events enablement.
    pub async fn create_hosted_link(
        &self,
        user: &str,
        product: PlaidProduct,
        flow: &str,
        access: Option<&AccessToken>,
    ) -> Result<HostedLink, PlaidError> {
        validate_opaque(user)?;
        let flow = uuid::Uuid::parse_str(flow).map_err(|_| PlaidError::Configuration)?;
        let mut completion = self.config.redirect_uri.clone();
        completion
            .query_pairs_mut()
            .append_pair("plaidFlow", &flow.to_string());
        let mut body = json!({"client_name":"Wealthfolio", "language":"en", "country_codes":["US"],
            "user":{"client_user_id":user}, "redirect_uri":self.config.redirect_uri.as_str(),
            "hosted_link":{"completion_redirect_uri":completion.as_str(),"url_lifetime_seconds":1800},
            "enable_multi_item_link":false});
        if let Some(access) = access {
            body["access_token"] = json!(self.token(access)?);
        } else {
            body["products"] = json!([product]);
            if product == PlaidProduct::Transactions {
                body["transactions"] = json!({"days_requested":730});
            }
        }
        #[derive(Deserialize)]
        struct Response {
            link_token: String,
            expiration: chrono::DateTime<Utc>,
            hosted_link_url: String,
        }
        let response: Response = self.post("/link/token/create", body).await?;
        let url = Url::parse(&response.hosted_link_url).map_err(|_| PlaidError::InvalidResponse)?;
        if url.scheme() != "https"
            || url.host_str() != Some("secure.plaid.com")
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || !url.path().starts_with("/hl/")
            || response.expiration <= Utc::now()
        {
            return Err(PlaidError::InvalidResponse);
        }
        Ok(HostedLink {
            token: PlaidToken::new(self.config.environment, response.link_token)
                .map_err(|_| PlaidError::InvalidResponse)?,
            url: response.hosted_link_url,
            expiration: response.expiration,
        })
    }

    /// Retrieves only the result associated with the server's saved Link token.
    pub async fn hosted_result(
        &self,
        link: &LinkCredential,
        update: bool,
    ) -> Result<Option<PublicToken>, PlaidError> {
        let value: Value = self
            .post("/link/token/get", json!({"link_token":self.token(link)?}))
            .await?;
        if value.get("link_token").and_then(Value::as_str) != Some(link.expose_secret()) {
            return Err(PlaidError::InvalidResponse);
        }
        let sessions = value
            .get("link_sessions")
            .and_then(Value::as_array)
            .ok_or(PlaidError::InvalidResponse)?;
        if sessions.is_empty() {
            return Err(PlaidError::NotReady);
        }
        if sessions.len() != 1 {
            return Err(PlaidError::InvalidResponse);
        }
        let session = &sessions[0];
        if session.get("finished_at").is_none_or(Value::is_null) {
            return Err(PlaidError::NotReady);
        }
        let results = session
            .pointer("/results/item_add_results")
            .and_then(Value::as_array);
        if update {
            // Update mode keeps the existing Item/token; a completed clean exit is required.
            if session.pointer("/exit/error").is_some_and(|e| !e.is_null()) {
                return Err(PlaidError::ReconnectRequired);
            }
            let handed_off =
                session
                    .get("events")
                    .and_then(Value::as_array)
                    .is_some_and(|events| {
                        events.iter().any(|event| {
                            event.get("event_name").and_then(Value::as_str) == Some("HANDOFF")
                        })
                    });
            if !handed_off {
                return Err(PlaidError::NotReady);
            }
            return Ok(None);
        }
        let results = results.ok_or(PlaidError::NotReady)?;
        if results.len() != 1 {
            return Err(PlaidError::InvalidResponse);
        }
        let token = results[0]
            .get("public_token")
            .and_then(Value::as_str)
            .ok_or(PlaidError::InvalidResponse)?;
        Ok(Some(
            PublicToken::new(self.config.environment, token.to_string())
                .map_err(|_| PlaidError::InvalidResponse)?,
        ))
    }

    pub async fn accounts(&self, token: &AccessToken) -> Result<AccountsResponse, PlaidError> {
        self.post("/accounts/get", json!({"access_token":self.token(token)?}))
            .await
    }

    /// Fetches one account-scoped page. Empty initial history is not proof of freshness.
    pub async fn transactions_page(
        &self,
        token: &AccessToken,
        account_id: &str,
        cursor: Option<&str>,
    ) -> Result<TransactionsPage, PlaidError> {
        validate_opaque(account_id)?;
        if let Some(cursor) = cursor {
            if cursor.is_empty() || cursor.len() > 256 || cursor == "now" {
                return Err(PlaidError::Configuration);
            }
        }
        let mut body = json!({"access_token":self.token(token)?, "count":500, "options":{"account_id":account_id}});
        if let Some(cursor) = cursor {
            body["cursor"] = json!(cursor);
        }
        self.post("/transactions/sync", body).await
    }

    pub async fn holdings(
        &self,
        token: &AccessToken,
        account_id: &str,
    ) -> Result<HoldingsResponse, PlaidError> {
        validate_opaque(account_id)?;
        self.post(
            "/investments/holdings/get",
            json!({"access_token":self.token(token)?, "options":{"account_ids":[account_id]}}),
        )
        .await
    }

    pub async fn investments_page(
        &self,
        token: &AccessToken,
        account_id: &str,
        start: NaiveDate,
        end: NaiveDate,
        offset: u32,
    ) -> Result<InvestmentsPage, PlaidError> {
        validate_opaque(account_id)?;
        if start > end {
            return Err(PlaidError::Configuration);
        }
        self.post(
            "/investments/transactions/get",
            json!({"access_token":self.token(token)?, "start_date":start,
            "end_date":end, "options":{"account_ids":[account_id], "count":500, "offset":offset}}),
        )
        .await
    }

    pub async fn remove_item(&self, token: &AccessToken) -> Result<(), PlaidError> {
        #[derive(Deserialize)]
        struct Response {
            request_id: String,
        }
        let response: Response = self
            .post("/item/remove", json!({"access_token":self.token(token)?}))
            .await?;
        validate_opaque(&response.request_id).map_err(|_| PlaidError::InvalidResponse)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
