//! OAuth machinery for Reddit's "installed app" flow: authorization code ->
//! permanent refresh token -> bearer tokens, with HTTP Basic auth (client ID +
//! empty secret) on token requests.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{Config, data_dir};

/// Persisted credentials. Stored as JSON with 0600 permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStore {
    /// Stable per-device identifier for the app-only (user-less) grant.
    pub device_id: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Unix timestamp (seconds) after which the access token is stale.
    pub expires_at: Option<i64>,
    /// Scopes the user granted.
    pub scopes: Option<String>,
    /// Logged-in username, cached after login.
    pub username: Option<String>,
}

impl TokenStore {
    pub fn fresh() -> Self {
        Self {
            device_id: random_token(24),
            access_token: None,
            refresh_token: None,
            expires_at: None,
            scopes: None,
            username: None,
        }
    }

    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<TokenStore>(&text) {
                Ok(mut store) => {
                    if store.device_id.len() < 20 || store.device_id.len() > 30 {
                        store.device_id = random_token(24);
                    }
                    store
                }
                Err(e) => {
                    tracing::warn!(error = %e, "token store unreadable, starting fresh");
                    Self::fresh()
                }
            },
            Err(_) => Self::fresh(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn logged_in(&self) -> bool {
        self.refresh_token.is_some()
    }
}

pub fn token_store_path(cfg: &Config) -> Result<std::path::PathBuf, std::io::Error> {
    match &cfg.data_dir {
        Some(dir) => Ok(dir.join("tokens.json")),
        None => Ok(data_dir()?.join("tokens.json")),
    }
}

/// Random URL-safe token (used for OAuth `state` and the device ID).
pub fn random_token(n: usize) -> String {
    use rand::Rng;
    use rand::distr::Alphanumeric;
    let rng = rand::rng();
    rng.sample_iter(&Alphanumeric)
        .take(n)
        .map(char::from)
        .collect()
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the manual-consent authorization URL (the page the user opens in a
/// browser). `duration=permanent` is what issues a refresh token.
pub fn build_authorize_url(cfg: &Config, state: &str) -> String {
    format!(
        "{}/api/v1/authorize?client_id={}&response_type=code&state={}&redirect_uri={}&duration=permanent&scope={}",
        cfg.www_base,
        percent_encode(cfg.client_id.trim()),
        percent_encode(state),
        percent_encode(&cfg.redirect_uri),
        percent_encode(&cfg.scopes)
    )
}

/// Accept whatever the user pastes back after consenting: a bare code, a full
/// redirect URL (`dystopia://response?state=..&code=..`), or a URL containing
/// `code=` / `error=` query parameters.
pub fn extract_code(input: &str) -> Result<(Option<String>, String), String> {
    // Devtools copies may include the header name ("location") and tabs;
    // use the whitespace-delimited token that looks like the redirect URL.
    let input = input
        .split_whitespace()
        .find(|t| t.contains("code="))
        .unwrap_or_else(|| input.trim());
    // Reddit's consent JS sometimes appends a "#_" fragment; strip it first.
    let input = input.trim().split('#').next().unwrap_or("").trim();
    if input.is_empty() {
        return Err("empty input: paste the authorization code or the whole redirect URL".into());
    }
    if let Some(q) = input.split_once('?') {
        let query = q.1;
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = percent_decode(v);
            match k {
                "code" => code = Some(v),
                "state" => state = Some(v),
                "error" => error = Some(v),
                _ => {}
            }
        }
        if let Some(err) = error {
            return Err(format!(
                "Reddit denied the authorization (error={err}). Restart with reddit_login_start."
            ));
        }
        return code
            .filter(|c| !c.is_empty())
            .map(|c| Ok((state, c)))
            .unwrap_or_else(|| Err("redirect URL contained no `code` parameter".into()));
    }
    // Bare code: Reddit codes are URL-safe base64-ish tokens.
    if input.contains(' ')
        || !input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '=')
    {
        return Err(
            "could not find an authorization code: paste the `code` value or the full redirect URL"
                .into(),
        );
    }
    Ok((None, input.to_string()))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len() + 1
            && i + 2 < bytes.len() + 1
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16)),
                bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parsed token-endpoint response.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            client_id: "abc123XYZ".into(),
            redirect_uri: "dystopia://response".into(),
            scopes: "*".into(),
            ..Config::default()
        }
    }

    #[test]
    fn authorize_url_contains_required_params() {
        let url = build_authorize_url(&cfg(), "st&ate=");
        assert!(url.starts_with("https://www.reddit.com/api/v1/authorize?"));
        assert!(url.contains("client_id=abc123XYZ"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("state=st%26ate%3D"));
        assert!(url.contains("redirect_uri=dystopia%3A%2F%2Fresponse"));
        assert!(url.contains("duration=permanent"));
        assert!(url.contains("scope=%2A"));
    }

    #[test]
    fn extracts_code_from_redirect_url() {
        let (state, code) = extract_code("dystopia://response?state=xyz&code=4fJ9-s_k").unwrap();
        assert_eq!(state.as_deref(), Some("xyz"));
        assert_eq!(code, "4fJ9-s_k");
    }

    #[test]
    fn strips_fragment_suffix() {
        let (state, code) = extract_code("dystopia://response?state=xyz&code=4fJ9-s_k#_").unwrap();
        assert_eq!(state.as_deref(), Some("xyz"));
        assert_eq!(code, "4fJ9-s_k");
    }

    #[test]
    fn extracts_bare_code() {
        let (state, code) = extract_code("  4fJ9-s_k ").unwrap();
        assert_eq!(state, None);
        assert_eq!(code, "4fJ9-s_k");
    }

    #[test]
    fn rejects_error_redirect() {
        let err =
            extract_code("redreader://rr_oauth_redir?state=x&error=access_denied").unwrap_err();
        assert!(err.contains("access_denied"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(extract_code("hello world").is_err());
    }
}
