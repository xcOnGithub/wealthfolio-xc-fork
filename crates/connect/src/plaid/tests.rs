use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

fn config() -> PlaidConfig {
    PlaidConfig::when_enabled(
        true,
        "sandbox",
        "test-client",
        "test-secret",
        "https://example.test/plaid/callback",
    )
    .unwrap()
    .unwrap()
}

fn token() -> AccessToken {
    AccessToken::new(PlaidEnvironment::Sandbox, "access-test".into()).unwrap()
}

async fn mock(status: u16, body: String) -> (PlaidClient, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = PlaidClient::new(config()).unwrap();
    client.base_url = format!("http://{}", listener.local_addr().unwrap());
    client.http = wealthfolio_http::client_builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(2))
        .redirect(Policy::none())
        .build()
        .unwrap();
    let worker = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut buf = [0; 4096];
            let size = socket.read(&mut buf).await.unwrap();
            assert!(size > 0, "request must complete");
            request.extend_from_slice(&buf[..size]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let len = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                if request.len() >= end + 4 + len {
                    break;
                }
            }
        }
        let response = format!("HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        // Oversized responses may be intentionally abandoned by the client.
        let _ = socket.write_all(response.as_bytes()).await;
        String::from_utf8(request).unwrap()
    });
    (client, worker)
}

fn request_body(request: &str, path: &str) -> Value {
    let (headers, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with(&format!("POST {path} HTTP/1.1\r\n")));
    assert!(!headers.lines().next().unwrap().contains("test-secret"));
    let lower = headers.to_ascii_lowercase();
    assert!(lower.contains("plaid-client-id: test-client\r\n"));
    assert!(lower.contains("plaid-secret: test-secret\r\n"));
    assert!(lower.contains("plaid-version: 2020-09-14\r\n"));
    let body: Value = serde_json::from_str(body).unwrap();
    assert!(body.get("secret").is_none());
    assert!(body.get("client_id").is_none());
    body
}

fn item() -> Value {
    json!({"item_id":"item-test", "institution_id":"ins-test", "available_products":["transactions"], "billed_products":["investments"]})
}

fn account() -> Value {
    json!({"account_id":"account-test", "name":"Synthetic", "type":"credit", "subtype":"credit card",
        "balances":{"current":100.25,"available":null,"iso_currency_code":"USD"}})
}

#[test]
fn disabled_and_invalid_configuration_are_inert() {
    assert!(PlaidConfig::when_enabled(false, "bad", "", "", "bad")
        .unwrap()
        .is_none());
    for env in [
        "",
        "development",
        "Production",
        "production/other",
        "https://other.test",
    ] {
        assert_eq!(
            env.parse::<PlaidEnvironment>(),
            Err(PlaidError::Configuration)
        );
    }
    assert_eq!(
        "production".parse::<PlaidEnvironment>().unwrap().base_url(),
        "https://production.plaid.com"
    );
    assert_eq!(
        "sandbox".parse::<PlaidEnvironment>().unwrap().base_url(),
        "https://sandbox.plaid.com"
    );
    for uri in [
        "bad",
        "http://example.test",
        "https://user@example.test",
        "https://user:pass@example.test",
        "https://:pass@example.test",
        "https://example.test/?x=1",
        "https://example.test/#token",
        "file:///tmp/test",
    ] {
        assert!(PlaidConfig::when_enabled(true, "sandbox", "id", "secret", uri).is_err());
    }
    for invalid in [
        "".to_string(),
        "a b".into(),
        "a\nb".into(),
        "x".repeat(16385),
    ] {
        assert!(AccessToken::new(PlaidEnvironment::Sandbox, invalid.clone()).is_err());
        for (id, secret) in [(invalid.as_str(), "secret"), ("id", invalid.as_str())] {
            assert!(PlaidConfig::when_enabled(
                true,
                "sandbox",
                id,
                secret,
                "https://example.test/"
            )
            .is_err());
        }
    }
    assert!(
        PlaidConfig::when_enabled(true, "bad", "id", "secret", "https://example.test/").is_err()
    );
}

#[test]
fn debugging_and_errors_are_redacted() {
    let cfg = config();
    assert!(!format!("{cfg:?}").contains("test-secret"));
    assert!(cfg.headers["plaid-secret"].is_sensitive());
    assert!(cfg.headers["plaid-client-id"].is_sensitive());
    let client = PlaidClient::new(cfg).unwrap();
    assert!(!format!("{client:?}").contains("test-client"));
    let token = token();
    assert_eq!(token.expose_secret(), "access-test");
    assert!(!format!("{token:?}").contains("access-test"));
    for error in [
        PlaidError::Configuration,
        PlaidError::Credentials,
        PlaidError::ReconnectRequired,
        PlaidError::RateLimited,
        PlaidError::NotReady,
        PlaidError::PaginationChanged,
        PlaidError::Unavailable,
        PlaidError::InvalidResponse,
        PlaidError::Rejected,
    ] {
        assert!(!error.to_string().is_empty());
        assert!(!format!("{error:?}: {error}").contains("access-test"));
    }
}

#[tokio::test]
async fn link_new_and_update_use_minimum_products() {
    for product in [PlaidProduct::Transactions, PlaidProduct::Investments] {
        let (client, server) = mock(
            200,
            json!({"link_token":"link-test", "expiration":"2026-12-01T00:00:00Z"}).to_string(),
        )
        .await;
        let linked = client
            .create_link_token("owner-profile-opaque", product)
            .await
            .unwrap();
        assert_eq!(linked.link_token.expose_secret(), "link-test");
        assert_eq!(linked.expiration.to_rfc3339(), "2026-12-01T00:00:00+00:00");
        let body = request_body(&server.await.unwrap(), "/link/token/create");
        assert_eq!(body["country_codes"], json!(["US"]));
        assert_eq!(body["products"], serde_json::to_value([product]).unwrap());
        assert_eq!(body["redirect_uri"], "https://example.test/plaid/callback");
        assert_eq!(body["user"]["client_user_id"], "owner-profile-opaque");
        assert!(body.get("access_token").is_none());
    }
    let (client, server) = mock(
        200,
        json!({"link_token":"update-link-test", "expiration":"2026-12-01T00:00:00Z"}).to_string(),
    )
    .await;
    client
        .create_update_link_token("owner", &token())
        .await
        .unwrap();
    let body = request_body(&server.await.unwrap(), "/link/token/create");
    assert!(body.get("products").is_none());
    assert_eq!(body["access_token"], "access-test");
}

#[tokio::test]
async fn exchange_and_revoke_have_explicit_credentials() {
    let (client, server) = mock(
        200,
        json!({"item_id":"item-test", "access_token":"new-access"}).to_string(),
    )
    .await;
    let public = PublicToken::new(PlaidEnvironment::Sandbox, "public-test".into()).unwrap();
    let result = client.exchange_public_token(&public).await.unwrap();
    assert_eq!(result.item_id, "item-test");
    assert_eq!(result.access_token.expose_secret(), "new-access");
    let body = request_body(&server.await.unwrap(), "/item/public_token/exchange");
    assert_eq!(body, json!({"public_token":"public-test"}));
    let (client, server) = mock(200, json!({"request_id":"safe-request"}).to_string()).await;
    client.remove_item(&token()).await.unwrap();
    assert_eq!(
        request_body(&server.await.unwrap(), "/item/remove"),
        json!({"access_token":"access-test"})
    );
}

#[tokio::test]
async fn accounts_preserve_nullable_balances_and_types() {
    let (client, server) = mock(
        200,
        json!({"accounts":[account()], "item":item()}).to_string(),
    )
    .await;
    let response = client.accounts(&token()).await.unwrap();
    let account = &response.accounts[0];
    assert_eq!(account.account_type, "credit");
    assert_eq!(account.subtype.as_deref(), Some("credit card"));
    assert_eq!(account.balances.current.unwrap().to_string(), "100.25");
    assert!(account.balances.available.is_none());
    assert_eq!(response.item.institution_id.as_deref(), Some("ins-test"));
    assert_eq!(
        request_body(&server.await.unwrap(), "/accounts/get"),
        json!({"access_token":"access-test"})
    );
}

#[tokio::test]
async fn transactions_keep_changes_and_account_scoped_cursor() {
    for cursor in [None, Some("opaque-cursor")] {
        let tx = json!({"account_id":"account-test", "transaction_id":"posted", "pending_transaction_id":"pending", "pending":false,
            "date":"2026-11-01", "amount":-0.25, "iso_currency_code":"USD", "name":"Synthetic"});
        let response = json!({"added":[tx.clone()], "modified":[tx], "removed":[{"account_id":"account-test","transaction_id":"removed"}],
            "next_cursor":"next", "has_more":true, "transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"});
        let (client, server) = mock(200, response.to_string()).await;
        let page = client
            .transactions_page(&token(), "account-test", cursor)
            .await
            .unwrap();
        assert!(page.has_more);
        assert_eq!(page.next_cursor, "next");
        assert_eq!(page.added[0].amount.to_string(), "-0.25");
        assert_eq!(
            page.modified[0].pending_transaction_id.as_deref(),
            Some("pending")
        );
        assert_eq!(page.removed[0].transaction_id, "removed");
        let body = request_body(&server.await.unwrap(), "/transactions/sync");
        assert_eq!(body["options"], json!({"account_id":"account-test"}));
        assert_eq!(body["count"], 500);
        assert_eq!(body.get("cursor").and_then(Value::as_str), cursor);
    }
    let (client, server) = mock(
        200,
        json!({"added":[], "modified":[], "removed":[], "next_cursor":"", "has_more":false})
            .to_string(),
    )
    .await;
    let page = client
        .transactions_page(&token(), "account-test", None)
        .await
        .unwrap();
    assert!(page.added.is_empty());
    assert!(!page.has_more);
    assert!(page.transactions_update_status.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn investments_preserve_unknown_types_missing_basis_and_precise_decimals() {
    let security = json!({"security_id":"security-test", "ticker_symbol":null, "type":"future-provider-type", "option_contract":{"contract_type":"call"}});
    let holding: Value = serde_json::from_str(r#"{"account_id":"account-test","security_id":"security-test","quantity":0.1234567890123456789,"institution_price":0,"institution_value":1.23,"cost_basis":null}"#).unwrap();
    let (client, server) = mock(200, json!({"accounts":[account()], "item":item(), "holdings":[holding], "securities":[security.clone()]}).to_string()).await;
    let response = client.holdings(&token(), "account-test").await.unwrap();
    assert_eq!(
        response.holdings[0].quantity.to_string(),
        "0.1234567890123456789"
    );
    assert!(response.holdings[0].cost_basis.is_none());
    assert!(response.securities[0].ticker_symbol.is_none());
    assert_eq!(
        response.securities[0].security_type.as_deref(),
        Some("future-provider-type")
    );
    assert_eq!(
        request_body(&server.await.unwrap(), "/investments/holdings/get")["options"],
        json!({"account_ids":["account-test"]})
    );
    let tx = json!({"investment_transaction_id":"sell-test", "account_id":"account-test", "security_id":null, "cancel_transaction_id":"old-test",
        "date":"2026-11-01", "name":"Synthetic", "type":"sell", "subtype":"sell", "amount":-12.34, "quantity":-1.25, "price":10, "fees":null});
    let (client, server) = mock(200, json!({"accounts":[], "item":item(), "investment_transactions":[tx], "securities":[security], "total_investment_transactions":501}).to_string()).await;
    let start = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    let response = client
        .investments_page(&token(), "account-test", start, end, 500)
        .await
        .unwrap();
    assert_eq!(response.total_investment_transactions, 501);
    assert_eq!(
        response.investment_transactions[0].amount.to_string(),
        "-12.34"
    );
    assert_eq!(
        response.investment_transactions[0]
            .cancel_transaction_id
            .as_deref(),
        Some("old-test")
    );
    let body = request_body(&server.await.unwrap(), "/investments/transactions/get");
    assert_eq!(body["start_date"], "2026-10-01");
    assert_eq!(body["end_date"], "2026-11-01");
    assert_eq!(
        body["options"],
        json!({"account_ids":["account-test"], "count":500, "offset":500})
    );
}

#[tokio::test]
async fn invalid_inputs_and_wrong_environments_make_no_requests() {
    let client = PlaidClient::new(config()).unwrap();
    let wrong = AccessToken::new(PlaidEnvironment::Production, "production-access".into()).unwrap();
    assert!(matches!(
        client.accounts(&wrong).await,
        Err(PlaidError::Configuration)
    ));
    assert!(matches!(
        client
            .create_link_token("", PlaidProduct::Transactions)
            .await,
        Err(PlaidError::Configuration)
    ));
    assert!(matches!(
        client.create_update_link_token("", &token()).await,
        Err(PlaidError::Configuration)
    ));
    assert!(matches!(
        client.transactions_page(&token(), "", None).await,
        Err(PlaidError::Configuration)
    ));
    for cursor in ["".to_string(), "now".into(), "x".repeat(257)] {
        assert!(matches!(
            client
                .transactions_page(&token(), "account", Some(&cursor))
                .await,
            Err(PlaidError::Configuration)
        ));
    }
    assert!(matches!(
        client.holdings(&token(), "").await,
        Err(PlaidError::Configuration)
    ));
    let date = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    assert!(matches!(
        client.investments_page(&token(), "", date, date, 0).await,
        Err(PlaidError::Configuration)
    ));
    assert!(matches!(
        client
            .investments_page(&token(), "account", date, date.pred_opt().unwrap(), 0)
            .await,
        Err(PlaidError::Configuration)
    ));
}

#[tokio::test]
async fn provider_failures_are_classified_without_echoing_payloads() {
    for (status, code, expected) in [
        (400, "INVALID_API_KEYS", PlaidError::Credentials),
        (400, "INVALID_ACCESS_TOKEN", PlaidError::Credentials),
        (400, "ITEM_LOGIN_REQUIRED", PlaidError::ReconnectRequired),
        (400, "ITEM_LOCKED", PlaidError::ReconnectRequired),
        (
            400,
            "USER_PERMISSION_REVOKED",
            PlaidError::ReconnectRequired,
        ),
        (400, "ITEM_NOT_SUPPORTED", PlaidError::ReconnectRequired),
        (400, "PRODUCT_NOT_READY", PlaidError::NotReady),
        (400, "NO_ACCOUNTS", PlaidError::NotReady),
        (
            400,
            "TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION",
            PlaidError::PaginationChanged,
        ),
        (400, "RATE_LIMIT_EXCEEDED", PlaidError::RateLimited),
        (429, "other", PlaidError::RateLimited),
        (503, "other", PlaidError::Unavailable),
        (403, "OTHER", PlaidError::Rejected),
    ] {
        let (client, server) = mock(status, json!({"error_code":code,"error_message":"access-test test-secret private account data"}).to_string()).await;
        let error = client.accounts(&token()).await.err().unwrap();
        assert_eq!(error, expected);
        assert!(!format!("{error:?}: {error}").contains("access-test"));
        server.await.unwrap();
    }
    for status in [302, 401, 404] {
        let (client, server) = mock(status, "private account data".into()).await;
        assert!(matches!(
            client.accounts(&token()).await,
            Err(PlaidError::Rejected)
        ));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn malformed_success_payloads_fail_closed() {
    for body in ["not json", "{}", r#"{"accounts":[],"item":null}"#] {
        let (client, server) = mock(200, body.into()).await;
        assert!(matches!(
            client.accounts(&token()).await,
            Err(PlaidError::InvalidResponse)
        ));
        server.await.unwrap();
    }
    for (item_id, access_token) in [("", "access"), ("item", "")] {
        let (client, server) = mock(
            200,
            json!({"item_id":item_id, "access_token":access_token}).to_string(),
        )
        .await;
        let public = PublicToken::new(PlaidEnvironment::Sandbox, "public-test".into()).unwrap();
        assert!(matches!(
            client.exchange_public_token(&public).await,
            Err(PlaidError::InvalidResponse)
        ));
        server.await.unwrap();
    }
    let (client, server) = mock(
        200,
        json!({"link_token":"", "expiration":"2026-12-01T00:00:00Z"}).to_string(),
    )
    .await;
    assert!(matches!(
        client
            .create_link_token("owner", PlaidProduct::Transactions)
            .await,
        Err(PlaidError::InvalidResponse)
    ));
    server.await.unwrap();
    let (client, server) = mock(200, json!({"request_id":""}).to_string()).await;
    assert_eq!(
        client.remove_item(&token()).await,
        Err(PlaidError::InvalidResponse)
    );
    server.await.unwrap();
    let (client, server) = mock(200, "x".repeat(MAX_RESPONSE_BYTES + 1)).await;
    assert!(matches!(
        client.accounts(&token()).await,
        Err(PlaidError::InvalidResponse)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn transport_timeout_and_truncated_body_are_sanitized() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = PlaidClient::new(config()).unwrap();
    client.base_url = format!("http://{}", listener.local_addr().unwrap());
    client.http = wealthfolio_http::client_builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    // Listener accepts no work: headers never arrive before the request deadline.
    assert!(matches!(
        client.accounts(&token()).await,
        Err(PlaidError::Unavailable)
    ));
    drop(listener);
    assert!(matches!(
        client.accounts(&token()).await,
        Err(PlaidError::Unavailable)
    ));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    client.base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{}")
            .await
            .unwrap();
    });
    assert!(matches!(
        client.accounts(&token()).await,
        Err(PlaidError::Unavailable)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn redirect_is_not_followed_and_credentials_do_not_leave_the_origin() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = PlaidClient::new(config()).unwrap();
    client.base_url = format!("http://{}", source.local_addr().unwrap());
    // Keep the production client's redirect policy; only remove proxy for loopback.
    client.http = wealthfolio_http::client_builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .timeout(TIMEOUT)
        .redirect(Policy::none())
        .build()
        .unwrap();
    let location = format!("http://{}/steal", target.local_addr().unwrap());
    let worker = tokio::spawn(async move {
        let (mut socket, _) = source.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket.write_all(format!("HTTP/1.1 307 Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    });
    assert!(matches!(
        client.accounts(&token()).await,
        Err(PlaidError::Rejected)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), target.accept())
            .await
            .is_err()
    );
    worker.await.unwrap();
}
