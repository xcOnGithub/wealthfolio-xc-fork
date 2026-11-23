use crate::{engine::invalid, model::*};
use async_trait::async_trait;
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use wealthfolio_core::{secrets::SecretStore, Result};

#[async_trait]
pub trait FamilyDelivery: Send + Sync {
    fn email_configured(&self, secrets: &dyn SecretStore) -> bool;
    fn default_sender(&self) -> Option<String> {
        None
    }
    async fn send(
        &self,
        record: &DeliveryRecord,
        settings: &FamilySettings,
        secrets: &dyn SecretStore,
    ) -> Result<()>;
}
pub struct HttpFamilyDelivery {
    fallback_key: Option<String>,
    fallback_sender: Option<String>,
}
impl HttpFamilyDelivery {
    pub fn new(fallback_key: Option<String>, fallback_sender: Option<String>) -> Self {
        Self {
            fallback_key,
            fallback_sender,
        }
    }
    fn email_key(&self, secrets: &dyn SecretStore) -> Option<String> {
        secrets
            .get_secret(RESEND_KEY)
            .ok()
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.fallback_key.clone())
    }
}
pub fn validate_webhook_url(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw).map_err(|_| invalid("Invalid webhook URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "Webhook must use HTTPS without URL credentials or fragments",
        ));
    }
    let host = url.host_str().unwrap_or_default();
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| !public_ip(ip))
    {
        return Err(invalid("Webhook must use a public network address"));
    }
    Ok(url)
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 192 && b == 0 && (c == 0 || c == 2))
                && !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                && !(a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(ip));
            }
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] == 0xdb8 || s[1] == 0))
                && s[0] != 0x2002
        }
    }
}
#[async_trait]
impl FamilyDelivery for HttpFamilyDelivery {
    fn email_configured(&self, secrets: &dyn SecretStore) -> bool {
        self.email_key(secrets).is_some()
    }
    fn default_sender(&self) -> Option<String> {
        self.fallback_sender.clone()
    }
    async fn send(
        &self,
        r: &DeliveryRecord,
        s: &FamilySettings,
        secrets: &dyn SecretStore,
    ) -> Result<()> {
        let builder = wealthfolio_http::client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15));
        let response = if r.channel == "email" {
            let key = self
                .email_key(secrets)
                .ok_or_else(|| invalid("Email provider is not configured"))?;
            builder.build().map_err(|_|invalid("Delivery client unavailable"))?
                .post("https://api.resend.com/emails").bearer_auth(key).header("Idempotency-Key",format!("family/{}",r.id))
                .json(&serde_json::json!({"from":s.email_from,"to":s.recipients,"subject":r.notification.subject,"text":r.notification.text}))
                .send().await.map_err(|_|invalid("Email delivery failed; check receipt before retrying"))?
        } else if r.channel == "webhook" {
            let endpoint = secrets
                .get_secret(WEBHOOK_URL_KEY)
                .map_err(|_| invalid("Webhook address unavailable"))?
                .ok_or_else(|| invalid("Webhook is not configured"))?;
            let url = validate_webhook_url(&endpoint)?;
            let host = url
                .host_str()
                .ok_or_else(|| invalid("Webhook host is missing"))?;
            let port = url.port_or_known_default().unwrap_or(443);
            let addresses: Vec<SocketAddr> = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::lookup_host((host, port)),
            )
            .await
            .map_err(|_| invalid("Webhook DNS lookup timed out"))?
            .map_err(|_| invalid("Webhook DNS lookup failed"))?
            .collect();
            if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
                return Err(invalid("Webhook resolved to a non-public address"));
            }
            let client = builder
                .no_proxy()
                .resolve_to_addrs(host, &addresses)
                .build()
                .map_err(|_| invalid("Delivery client unavailable"))?;
            let mut request=client.post(url.clone()).header("Idempotency-Key",format!("family/{}",r.id))
                .json(&serde_json::json!({"type":"family.notification","id":r.id,"createdAt":r.created_at,"notification":r.notification}));
            if let Some(token) = secrets
                .get_secret(WEBHOOK_TOKEN_KEY)
                .map_err(|_| invalid("Webhook credential unavailable"))?
            {
                if !token.is_empty() {
                    request = request.bearer_auth(token);
                }
            }
            request
                .send()
                .await
                .map_err(|_| invalid("Webhook delivery failed; check receipt before retrying"))?
        } else {
            return Err(invalid("Unknown delivery channel"));
        };
        if !response.status().is_success() {
            return Err(invalid(
                "Delivery was not accepted; check provider configuration",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn webhook_rejects_local_and_credential_urls() {
        for url in [
            "http://example.com/hook",
            "https://localhost/hook",
            "https://127.0.0.1",
            "https://169.254.169.254/latest",
            "https://[::1]",
            "https://user:password@example.com",
        ] {
            assert!(validate_webhook_url(url).is_err(), "{url}");
        }
        for ip in [
            "10.0.0.1",
            "100.64.0.1",
            "198.18.0.1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(validate_webhook_url("https://example.com/hook").is_ok());
    }
}
