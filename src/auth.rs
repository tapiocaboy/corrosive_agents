//! Authentication for agent transports (features `server` / `grpc`).
//!
//! One [`AuthScheme`] protects both REST/WebSocket
//! ([`server::router_with_auth`](crate::server::router_with_auth)) and gRPC
//! ([`grpc::serve_with_auth`](crate::grpc::serve_with_auth)):
//!
//! - **API keys** — clients send `Authorization: Bearer <key>` or
//!   `X-Api-Key: <key>`.
//! - **JWT (HS256)** — shared-secret HMAC-SHA256 tokens.
//! - **JWT (RS256 + JWKS)** — RSA tokens verified against a JSON Web Key
//!   Set, e.g. from your identity provider's
//!   `/.well-known/jwks.json` ([`JwksStore`]).
//!
//! All JWTs must carry a future `exp` and, when configured, matching
//! `iss`/`aud` claims. Health/readiness endpoints stay unauthenticated so
//! probes keep working.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::{Error, Result};

/// Allowed clock skew when checking `exp`/`nbf` (seconds).
const LEEWAY_SECS: u64 = 30;

/// How clients must authenticate.
#[derive(Clone)]
pub enum AuthScheme {
    /// A fixed set of accepted API keys.
    ApiKeys(HashSet<String>),
    /// HS256-signed JWTs verified with a shared secret.
    JwtHs256 {
        /// Shared HMAC secret.
        secret: String,
        /// Required `iss` claim, when set.
        issuer: Option<String>,
        /// Required `aud` claim, when set.
        audience: Option<String>,
    },
    /// RS256-signed JWTs verified against a JSON Web Key Set.
    JwtRs256 {
        /// The RSA public keys (kid → key), loadable from JWKS JSON or URL.
        keys: Arc<JwksStore>,
        /// Required `iss` claim, when set.
        issuer: Option<String>,
        /// Required `aud` claim, when set.
        audience: Option<String>,
    },
}

impl std::fmt::Debug for AuthScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        match self {
            Self::ApiKeys(keys) => f
                .debug_struct("AuthScheme::ApiKeys")
                .field("count", &keys.len())
                .finish(),
            Self::JwtHs256 {
                issuer, audience, ..
            } => f
                .debug_struct("AuthScheme::JwtHs256")
                .field("issuer", issuer)
                .field("audience", audience)
                .finish_non_exhaustive(),
            Self::JwtRs256 {
                keys,
                issuer,
                audience,
            } => f
                .debug_struct("AuthScheme::JwtRs256")
                .field("keys", &keys.len())
                .field("issuer", issuer)
                .field("audience", audience)
                .finish_non_exhaustive(),
        }
    }
}

/// One RSA public key from a JWKS document (raw big-endian components).
#[derive(Clone)]
struct RsaJwk {
    n: Vec<u8>,
    e: Vec<u8>,
}

/// A set of RSA public keys for RS256 verification, keyed by `kid`.
///
/// Load once from static JWKS JSON ([`from_json`](Self::from_json)) or from
/// a URL ([`from_url`](Self::from_url)); call [`refresh`](Self::refresh) on
/// key rotation (e.g. from a periodic task). Verification itself is
/// synchronous against the cached keys.
pub struct JwksStore {
    url: Option<String>,
    keys: std::sync::RwLock<HashMap<String, RsaJwk>>,
}

impl std::fmt::Debug for JwksStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwksStore")
            .field("url", &self.url)
            .field("keys", &self.len())
            .finish()
    }
}

impl JwksStore {
    /// Parse a static JWKS document (`{"keys":[{"kty":"RSA",…}]}`).
    /// Non-RSA keys and keys marked for encryption (`"use":"enc"`) are
    /// skipped.
    pub fn from_json(jwks: &str) -> Result<Self> {
        let keys = Self::parse(jwks)?;
        if keys.is_empty() {
            return Err(Error::Auth("JWKS contains no usable RSA keys".into()));
        }
        Ok(Self {
            url: None,
            keys: std::sync::RwLock::new(keys),
        })
    }

    /// Fetch a JWKS document from `url` (typically
    /// `https://<issuer>/.well-known/jwks.json`).
    pub async fn from_url(url: impl Into<String>) -> Result<Self> {
        let url = url.into();
        let store = Self {
            url: Some(url),
            keys: std::sync::RwLock::new(HashMap::new()),
        };
        store.refresh().await?;
        Ok(store)
    }

    /// Re-fetch the JWKS from the configured URL, replacing the cached keys.
    /// Returns the number of keys loaded. No-op error for JSON-loaded stores.
    pub async fn refresh(&self) -> Result<usize> {
        let url = self
            .url
            .as_deref()
            .ok_or_else(|| Error::Auth("this JWKS store was loaded from static JSON".into()))?;
        let body = reqwest::get(url)
            .await
            .map_err(|e| Error::Auth(format!("JWKS fetch from {url} failed: {e}")))?
            .text()
            .await
            .map_err(|e| Error::Auth(format!("JWKS fetch from {url} failed: {e}")))?;
        let keys = Self::parse(&body)?;
        if keys.is_empty() {
            return Err(Error::Auth(format!(
                "JWKS at {url} contains no usable RSA keys"
            )));
        }
        let count = keys.len();
        *self.keys.write().expect("jwks lock poisoned") = keys;
        Ok(count)
    }

    /// Number of cached keys.
    pub fn len(&self) -> usize {
        self.keys.read().expect("jwks lock poisoned").len()
    }

    /// `true` when no keys are cached.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn parse(jwks: &str) -> Result<HashMap<String, RsaJwk>> {
        let document: serde_json::Value = serde_json::from_str(jwks)
            .map_err(|e| Error::Auth(format!("invalid JWKS JSON: {e}")))?;
        let mut keys = HashMap::new();
        for (index, key) in document
            .get("keys")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let get = |field: &str| key.get(field).and_then(serde_json::Value::as_str);
            if get("kty") != Some("RSA") || get("use").is_some_and(|u| u != "sig") {
                continue;
            }
            if get("alg").is_some_and(|a| a != "RS256") {
                continue;
            }
            let (Some(n), Some(e)) = (get("n"), get("e")) else {
                continue;
            };
            let n = URL_SAFE_NO_PAD
                .decode(n)
                .map_err(|_| Error::Auth("invalid base64url modulus in JWKS".into()))?;
            let e = URL_SAFE_NO_PAD
                .decode(e)
                .map_err(|_| Error::Auth("invalid base64url exponent in JWKS".into()))?;
            let kid = get("kid")
                .map(String::from)
                .unwrap_or_else(|| index.to_string());
            keys.insert(kid, RsaJwk { n, e });
        }
        Ok(keys)
    }

    /// Look up by `kid`; a token without `kid` matches only a single-key set.
    fn get(&self, kid: Option<&str>) -> Option<RsaJwk> {
        let keys = self.keys.read().expect("jwks lock poisoned");
        match kid {
            Some(kid) => keys.get(kid).cloned(),
            None if keys.len() == 1 => keys.values().next().cloned(),
            None => None,
        }
    }
}

impl AuthScheme {
    /// Accept any of the given API keys.
    pub fn api_keys<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::ApiKeys(keys.into_iter().map(Into::into).collect())
    }

    /// Accept HS256 JWTs signed with `secret` (tokens must carry `exp`).
    pub fn jwt_hs256(secret: impl Into<String>) -> Self {
        Self::JwtHs256 {
            secret: secret.into(),
            issuer: None,
            audience: None,
        }
    }

    /// Accept RS256 JWTs verified against a [`JwksStore`]
    /// (tokens must carry `exp`; `kid` selects the key).
    pub fn jwt_rs256(keys: JwksStore) -> Self {
        Self::JwtRs256 {
            keys: Arc::new(keys),
            issuer: None,
            audience: None,
        }
    }

    /// Additionally require the `iss` claim to equal `issuer`.
    #[must_use]
    pub fn with_issuer(self, issuer: impl Into<String>) -> Self {
        let issuer = Some(issuer.into());
        match self {
            Self::JwtHs256 {
                secret, audience, ..
            } => Self::JwtHs256 {
                secret,
                issuer,
                audience,
            },
            Self::JwtRs256 { keys, audience, .. } => Self::JwtRs256 {
                keys,
                issuer,
                audience,
            },
            other => other,
        }
    }

    /// Additionally require the `aud` claim to equal `audience`.
    #[must_use]
    pub fn with_audience(self, audience: impl Into<String>) -> Self {
        let audience = Some(audience.into());
        match self {
            Self::JwtHs256 { secret, issuer, .. } => Self::JwtHs256 {
                secret,
                issuer,
                audience,
            },
            Self::JwtRs256 { keys, issuer, .. } => Self::JwtRs256 {
                keys,
                issuer,
                audience,
            },
            other => other,
        }
    }

    /// Authorize a request given its raw `Authorization` header value and/or
    /// `X-Api-Key` header value.
    pub fn authorize(&self, authorization: Option<&str>, api_key: Option<&str>) -> Result<()> {
        let bearer = authorization
            .and_then(|v| {
                v.strip_prefix("Bearer ")
                    .or_else(|| v.strip_prefix("bearer "))
            })
            .map(str::trim);

        match self {
            Self::ApiKeys(keys) => match bearer.or(api_key) {
                Some(candidate) if keys.contains(candidate) => Ok(()),
                Some(_) => Err(Error::Auth("invalid API key".into())),
                None => Err(Error::Auth(
                    "missing credentials: send 'Authorization: Bearer <key>' or 'X-Api-Key'".into(),
                )),
            },
            Self::JwtHs256 {
                secret,
                issuer,
                audience,
            } => {
                let token = bearer.ok_or_else(|| {
                    Error::Auth("missing credentials: send 'Authorization: Bearer <jwt>'".into())
                })?;
                verify_jwt_hs256(token, secret, issuer.as_deref(), audience.as_deref())
            }
            Self::JwtRs256 {
                keys,
                issuer,
                audience,
            } => {
                let token = bearer.ok_or_else(|| {
                    Error::Auth("missing credentials: send 'Authorization: Bearer <jwt>'".into())
                })?;
                verify_jwt_rs256(token, keys, issuer.as_deref(), audience.as_deref())
            }
        }
    }
}

/// Split a compact JWT and parse its header; returns
/// `(header_json, header_b64, payload_b64, signature_bytes)`.
fn split_jwt(token: &str) -> Result<(serde_json::Value, &str, &str, Vec<u8>)> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Error::Auth("invalid JWT: expected three segments".into()));
    };
    let header_json: serde_json::Value = serde_json::from_slice(&b64url(header, "header")?)
        .map_err(|_| Error::Auth("invalid JWT: malformed header".into()))?;
    let signature = b64url(signature, "signature")?;
    Ok((header_json, header, payload, signature))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn b64url(part: &str, what: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| Error::Auth(format!("invalid JWT: {what} is not base64url")))
}

/// Validate an HS256 JWT: signature, `exp` (required), `nbf`, and optional
/// `iss`/`aud` claims.
fn verify_jwt_hs256(
    token: &str,
    secret: &str,
    issuer: Option<&str>,
    audience: Option<&str>,
) -> Result<()> {
    let (header_json, header, payload, signature) = split_jwt(token)?;
    if header_json.get("alg").and_then(|v| v.as_str()) != Some("HS256") {
        return Err(Error::Auth("invalid JWT: only HS256 is accepted".into()));
    }

    // Constant-time signature check over "<header>.<payload>".
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| Error::Auth("invalid JWT secret".into()))?;
    mac.update(header.as_bytes());
    mac.update(b".");
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| Error::Auth("invalid JWT: signature mismatch".into()))?;

    validate_claims(payload, issuer, audience)
}

/// Validate an RS256 JWT against a JWKS: signature (RSA PKCS#1 v1.5,
/// SHA-256), `exp` (required), `nbf`, and optional `iss`/`aud` claims.
fn verify_jwt_rs256(
    token: &str,
    keys: &JwksStore,
    issuer: Option<&str>,
    audience: Option<&str>,
) -> Result<()> {
    let (header_json, header, payload, signature) = split_jwt(token)?;
    if header_json.get("alg").and_then(|v| v.as_str()) != Some("RS256") {
        return Err(Error::Auth("invalid JWT: only RS256 is accepted".into()));
    }

    let kid = header_json.get("kid").and_then(|v| v.as_str());
    let key = keys.get(kid).ok_or_else(|| {
        Error::Auth(match kid {
            Some(kid) => format!("invalid JWT: no JWKS key with kid '{kid}' (try refreshing)"),
            None => "invalid JWT: token has no 'kid' and the JWKS is ambiguous".into(),
        })
    })?;

    let message = format!("{header}.{payload}");
    ring::signature::RsaPublicKeyComponents {
        n: &key.n,
        e: &key.e,
    }
    .verify(
        &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        message.as_bytes(),
        &signature,
    )
    .map_err(|_| Error::Auth("invalid JWT: signature mismatch".into()))?;

    validate_claims(payload, issuer, audience)
}

/// Shared registered-claims validation (`exp`, `nbf`, `iss`, `aud`).
fn validate_claims(payload: &str, issuer: Option<&str>, audience: Option<&str>) -> Result<()> {
    let claims: serde_json::Value = serde_json::from_slice(&b64url(payload, "payload")?)
        .map_err(|_| Error::Auth("invalid JWT: malformed claims".into()))?;

    let now = unix_now();
    let exp = claims
        .get("exp")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| Error::Auth("invalid JWT: missing 'exp' claim".into()))?;
    if exp + LEEWAY_SECS < now {
        return Err(Error::Auth("invalid JWT: token is expired".into()));
    }
    if let Some(nbf) = claims.get("nbf").and_then(|v| v.as_u64()) {
        if nbf > now + LEEWAY_SECS {
            return Err(Error::Auth("invalid JWT: token is not yet valid".into()));
        }
    }
    if let Some(expected) = issuer {
        if claims.get("iss").and_then(|v| v.as_str()) != Some(expected) {
            return Err(Error::Auth("invalid JWT: wrong issuer".into()));
        }
    }
    if let Some(expected) = audience {
        let aud = claims.get("aud");
        let matches = match aud {
            Some(serde_json::Value::String(s)) => s == expected,
            Some(serde_json::Value::Array(list)) => {
                list.iter().any(|v| v.as_str() == Some(expected))
            }
            _ => false,
        };
        if !matches {
            return Err(Error::Auth("invalid JWT: wrong audience".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_jwt(secret: &str, claims: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("{header}.{payload}").as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{header}.{payload}.{signature}")
    }

    fn future_exp() -> u64 {
        unix_now() + 3600
    }

    #[test]
    fn api_keys_accept_bearer_and_header() {
        let auth = AuthScheme::api_keys(["k1", "k2"]);
        auth.authorize(Some("Bearer k1"), None).unwrap();
        auth.authorize(None, Some("k2")).unwrap();
        assert!(auth.authorize(Some("Bearer nope"), None).is_err());
        assert!(auth.authorize(None, None).is_err());
    }

    #[test]
    fn jwt_validates_signature_and_claims() {
        let auth = AuthScheme::jwt_hs256("s3cret").with_issuer("corrosive");
        let good = make_jwt("s3cret", json!({ "exp": future_exp(), "iss": "corrosive" }));
        auth.authorize(Some(&format!("Bearer {good}")), None)
            .unwrap();

        let wrong_secret = make_jwt("other", json!({ "exp": future_exp(), "iss": "corrosive" }));
        assert!(auth
            .authorize(Some(&format!("Bearer {wrong_secret}")), None)
            .is_err());

        let wrong_issuer = make_jwt("s3cret", json!({ "exp": future_exp(), "iss": "evil" }));
        assert!(auth
            .authorize(Some(&format!("Bearer {wrong_issuer}")), None)
            .is_err());

        let expired = make_jwt("s3cret", json!({ "exp": 1000, "iss": "corrosive" }));
        assert!(auth
            .authorize(Some(&format!("Bearer {expired}")), None)
            .is_err());

        let missing_exp = make_jwt("s3cret", json!({ "iss": "corrosive" }));
        assert!(auth
            .authorize(Some(&format!("Bearer {missing_exp}")), None)
            .is_err());
    }

    #[test]
    fn jwt_audience_string_or_array() {
        let auth = AuthScheme::jwt_hs256("s").with_audience("agents");
        let single = make_jwt("s", json!({ "exp": future_exp(), "aud": "agents" }));
        auth.authorize(Some(&format!("Bearer {single}")), None)
            .unwrap();
        let list = make_jwt(
            "s",
            json!({ "exp": future_exp(), "aud": ["other", "agents"] }),
        );
        auth.authorize(Some(&format!("Bearer {list}")), None)
            .unwrap();
        let wrong = make_jwt("s", json!({ "exp": future_exp(), "aud": "nope" }));
        assert!(auth
            .authorize(Some(&format!("Bearer {wrong}")), None)
            .is_err());
    }

    #[test]
    fn non_hs256_rejected() {
        let auth = AuthScheme::jwt_hs256("s");
        // alg=none downgrade attempt
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(json!({ "exp": future_exp() }).to_string());
        let forged = format!("{header}.{payload}.");
        assert!(auth
            .authorize(Some(&format!("Bearer {forged}")), None)
            .is_err());
    }

    // ── RS256 / JWKS ─────────────────────────────────────────────────────

    /// A fixed RSA-2048 test key (PKCS#8 DER, base64). Test-only material.
    const TEST_RSA_PKCS8_B64: &str = "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCmKPH2Su6Z49De2egkDfwTXMLAhRQebihlfgI8C9qO0Ij5FCw2v9vb+0KKgfR4sbe1dS02IJ8dBSOlgMka8DIKoz8d9KdKe+EI4traqf0xpD/YxPh7JxeyVqZyMMrrt+XW4q3wLPoApGnsdZJwJVPTN+TkPTVA9x4D4EV3cHgUS+Volb7XSeKD5035pNjkl1KMP0zhhwDyTp+2OJKY6PnnFZ+dlbbVgPbknSnOMlRlSKsM3PmbMX3ek69ylcSfchgxvS7ACCw23OyYv5IesV2ElFa9cqXA3Thlv6auoT8LETMcbOCDrzaVE4ezp4nJozQaWg17yArwuUJQrHhUtmpVAgMBAAECggEAD7/pz3Ki0yto7Payrlg1AJDWVPFISuoeIiCjjZWCDe9uTE3BMx9Uc7GJSR+wUJBn3WdR9cN50YJfMpzWcxs5YxC+NtSt1r2PQwxdukRKn531/1IMS4AVGu5jsIc3dMhlnMy3uABLYiwzbhpm2wJuw6KUq52xoorJ6YwkiYG7oBC6gJ1LM2TSIL+iCGN7QuK57Wi/wcZ7646mDy2NBrHG7svM5axQkeUkyyTEp1HwbDECPbQGpd1pXCLqVJq8yy3MEDDbZ71JTfwclxbsSyUiioWGbsnEvzFlfbCKjRtT+KYrvNykp2pqArgCw9wlPB0GGWZyABR8gX0gJOSdbwszgQKBgQDPDqGsoIyxVKk+3R0E3AuPJrSHjKSrtnUBIPX6Y78eonByIyEwZ8uT5mEhqWGrbtcBSoROvXWMnIHXopT1BMc6uIdxWWqnvjtRlSBJgcQ25d+7GvOkshbDT9Z9muvoGCg8rVwhSfZJ4AxoSKgvnt9xeFL2+xERGR0EJBPi9FYWjQKBgQDNb45fAIFKq6AwvQZG1LQduYBWdByeAotAtmjUXW8wI2Y4dki/3g9x0SsPuRD1hslsfa22KY8jJDjPGH1fwxyjM8X+Uvh3pMAo+eqkWEsKnGUdWklzFsOgeg0WDV3tx8xDnrBqqLMYrF9qwZWkqkJuyw0MA388MTefLt9eqbR06QKBgQDMnJ3h1GoUFyCEscaqdbSqisodpTtZQJ3RNrw86nMEF+vcrqBukDOZ/TCBLjwJSCgJ65Rhp1HRWRvqdoySsF0cxxt5RK5kA1XlIePdH/JBedoksNaSKzbZXT0NtJlpKu4gQARqFQfgKxq3tw0Uuf/+xrPdw28zIUkOPYS1Y1TrRQKBgENxKxEfLlLgMw+tDoF0VMkpW+uF5NsuxJ5zA8kr/1OTW3yPwGRUt0dLPtLDk8C3Bis6uyuBSz9jJc8/H/GvMRiW55oNjQpiKL+LBC/92GzcWQmg2VoSEBj/2Inzy3FDVVihoRLy3RDtjcmTUdgkGPkcaeUWxM9y7OqyTZxbJCX5AoGAVIBP27/zLsnZKFbkV/FLJMkAJUYCGYJyx6KrWExRiSTFJhQjeLKzzD0XdGHL1uX0AXdi+Qx3aEYJLI0lyru4Hnsokb4C+d3PepsVi73Wlt/PQZMQ8PLvVUlg9x4DPZR+bGCdCDh9BXXoMntcEfLofJO1FooUumZgD/U+CXfC4B0=";
    const TEST_RSA_N_B64URL: &str = "pijx9krumePQ3tnoJA38E1zCwIUUHm4oZX4CPAvajtCI-RQsNr_b2_tCioH0eLG3tXUtNiCfHQUjpYDJGvAyCqM_HfSnSnvhCOLa2qn9MaQ_2MT4eycXslamcjDK67fl1uKt8Cz6AKRp7HWScCVT0zfk5D01QPceA-BFd3B4FEvlaJW-10nig-dN-aTY5JdSjD9M4YcA8k6ftjiSmOj55xWfnZW21YD25J0pzjJUZUirDNz5mzF93pOvcpXEn3IYMb0uwAgsNtzsmL-SHrFdhJRWvXKlwN04Zb-mrqE_CxEzHGzgg682lROHs6eJyaM0GloNe8gK8LlCUKx4VLZqVQ";

    fn test_jwks() -> String {
        json!({
            "keys": [{
                "kty": "RSA",
                "kid": "test-key",
                "use": "sig",
                "alg": "RS256",
                "n": TEST_RSA_N_B64URL,
                "e": "AQAB",
            }]
        })
        .to_string()
    }

    fn make_rs256_jwt(claims: serde_json::Value, kid: Option<&str>) -> String {
        use base64::engine::general_purpose::STANDARD;
        let der = STANDARD.decode(TEST_RSA_PKCS8_B64).unwrap();
        let key_pair = ring::signature::RsaKeyPair::from_pkcs8(&der).unwrap();

        let header = match kid {
            Some(kid) => json!({ "alg": "RS256", "typ": "JWT", "kid": kid }),
            None => json!({ "alg": "RS256", "typ": "JWT" }),
        };
        let header = URL_SAFE_NO_PAD.encode(header.to_string());
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let message = format!("{header}.{payload}");

        let mut signature = vec![0; key_pair.public().modulus_len()];
        key_pair
            .sign(
                &ring::signature::RSA_PKCS1_SHA256,
                &ring::rand::SystemRandom::new(),
                message.as_bytes(),
                &mut signature,
            )
            .unwrap();
        format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    #[test]
    fn rs256_jwks_roundtrip() {
        let auth =
            AuthScheme::jwt_rs256(JwksStore::from_json(&test_jwks()).unwrap()).with_issuer("idp");

        let good = make_rs256_jwt(
            json!({ "exp": future_exp(), "iss": "idp" }),
            Some("test-key"),
        );
        auth.authorize(Some(&format!("Bearer {good}")), None)
            .unwrap();

        // Single-key sets also match tokens without a kid.
        let no_kid = make_rs256_jwt(json!({ "exp": future_exp(), "iss": "idp" }), None);
        auth.authorize(Some(&format!("Bearer {no_kid}")), None)
            .unwrap();
    }

    #[test]
    fn rs256_rejects_tampering_and_wrong_kid() {
        let auth = AuthScheme::jwt_rs256(JwksStore::from_json(&test_jwks()).unwrap());

        let token = make_rs256_jwt(
            json!({ "exp": future_exp(), "role": "user" }),
            Some("test-key"),
        );
        // Tamper with the payload (escalate role) keeping the signature.
        let mut parts: Vec<&str> = token.split('.').collect();
        let forged_payload =
            URL_SAFE_NO_PAD.encode(json!({ "exp": future_exp(), "role": "admin" }).to_string());
        parts[1] = &forged_payload;
        let forged = parts.join(".");
        assert!(auth
            .authorize(Some(&format!("Bearer {forged}")), None)
            .is_err());

        let unknown_kid = make_rs256_jwt(json!({ "exp": future_exp() }), Some("other-key"));
        let err = auth
            .authorize(Some(&format!("Bearer {unknown_kid}")), None)
            .unwrap_err();
        assert!(err.to_string().contains("kid"), "got: {err}");

        let expired = make_rs256_jwt(json!({ "exp": 1000 }), Some("test-key"));
        assert!(auth
            .authorize(Some(&format!("Bearer {expired}")), None)
            .is_err());
    }

    #[test]
    fn hs256_token_rejected_by_rs256_scheme() {
        // Algorithm-confusion guard: an HS256 token signed with the public
        // modulus as the HMAC secret must not pass an RS256 scheme.
        let auth = AuthScheme::jwt_rs256(JwksStore::from_json(&test_jwks()).unwrap());
        let confused = make_jwt(TEST_RSA_N_B64URL, json!({ "exp": future_exp() }));
        assert!(auth
            .authorize(Some(&format!("Bearer {confused}")), None)
            .is_err());
    }

    #[test]
    fn jwks_parsing_skips_unusable_keys() {
        let jwks = json!({
            "keys": [
                { "kty": "EC", "kid": "ec-key", "crv": "P-256" },
                { "kty": "RSA", "kid": "enc-key", "use": "enc",
                  "n": TEST_RSA_N_B64URL, "e": "AQAB" },
                { "kty": "RSA", "kid": "good", "use": "sig",
                  "n": TEST_RSA_N_B64URL, "e": "AQAB" },
            ]
        })
        .to_string();
        let store = JwksStore::from_json(&jwks).unwrap();
        assert_eq!(store.len(), 1);
        assert!(store.get(Some("good")).is_some());
        assert!(store.get(Some("ec-key")).is_none());
    }
}
