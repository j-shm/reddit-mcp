//! Reddit HTTP client: token lifecycle (manual-paste OAuth code flow,
//! refresh, app-only grant), request pipeline (bearer auth, `raw_json`,
//! `api_type=json`), client-side rate limiting, and translation of Reddit's
//! error payloads into actionable messages.

use std::time::Duration;

use serde_json::Value;

use crate::auth::{self, TokenResponse, TokenStore};
use crate::config::{Config, mask_secret};
use crate::models;

#[derive(Debug, thiserror::Error)]
pub enum RedditError {
    #[error("not configured: set REDDIT_CLIENT_ID (the donor-app App ID) and restart the server")]
    NotConfigured,
    #[error(
        "not logged in: call reddit_login_start, complete the browser authorization, then reddit_login_complete"
    )]
    NotLoggedIn,
    #[error(
        "login required: this action needs a user token but only app-only (read) access is available"
    )]
    LoginRequired,
    #[error("auth error: {0}")]
    Auth(String),
    #[error("Reddit rejected the request: {0}")]
    Api(String),
    #[error("Reddit returned HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("rate limited by Reddit: retry after ~{0}s")]
    RateLimited(u64),
    #[error("network error talking to Reddit: {0}")]
    Network(String),
    #[error("unexpected Reddit response shape: {0}")]
    BadShape(String),
    #[error("local storage error: {0}")]
    Io(String),
}

impl RedditError {
    /// Extra hints appended to specific Reddit error codes.
    fn hint(code: &str) -> Option<&'static str> {
        Some(match code {
            "RATELIMIT" => "Wait the stated time, then retry the same call.",
            "BAD_CAPTCHA" => {
                "Reddit is demanding a CAPTCHA. Headless clients cannot solve these; wait and retry, or act from an account with more standing."
            }
            "ALREADY_SUB" => "That link was already submitted; pass resubmit=true to override.",
            "NOT_AUTHOR" => "You can only edit or delete your own content.",
            "DELETE_TIMELIMIT" => "Reddit throttles deletes; retry in about an hour.",
            "TOO_OLD" => "Reddit no longer allows editing content that old.",
            "THREAD_LOCKED" => "The thread is locked; commenting is disabled.",
            "USER_BANNED_FROM_SUBREDDIT" | "BANNED_FROM_SUBREDDIT" => {
                "The acting account is banned from that subreddit."
            }
            "MUTED" => "The account is muted and cannot message here.",
            "SUBREDDIT_NOEXIST" | "NO_SUBREDDIT" | "BAD_SR_NAME" => {
                "Check the subreddit name (no /r/ prefix needed)."
            }
            "PRIVATE_SUBREDDIT" => "That subreddit is private and the token lacks access.",
            "MODERATOR_REQUIRED" | "MODERATOR_OF_SUBREDDIT_REQUIRED" => {
                "This moderation action requires being a moderator of that subreddit (modpost/modlog scopes)."
            }
            "NO_INVITE_FOUND" => "No pending invite found for that user.",
            "QUOTA_FILLED" => "The subreddit's list for that role is full.",
            "FLAIR_TEMPLATEID_NOT_FOUND" => {
                "The flair template ID does not exist; list options first."
            }
            "TOO_MUCH_FLAIR_TEXT" => "Flair text too long (limit ~64 chars including CSS class).",
            "USER_REQUIRED" => "A logged-in user is required.",
            _ => return None,
        })
    }
}

fn format_api_errors(errors: &[Value]) -> String {
    let mut parts = Vec::new();
    for e in errors {
        let arr = e.as_array();
        let code = arr
            .and_then(|a| a.first())
            .and_then(|x| x.as_str())
            .unwrap_or("UNKNOWN");
        let msg = arr
            .and_then(|a| a.get(1))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let mut part = if msg.is_empty() {
            code.to_string()
        } else {
            format!("{code}: {msg}")
        };
        if let Some(hint) = RedditError::hint(code) {
            part.push_str(&format!(" — {hint}"));
        }
        parts.push(part);
    }
    parts.join("; ")
}

#[derive(Debug)]
struct AuthState {
    store: TokenStore,
    pending_state: Option<String>,
    app_only: Option<(String, i64)>,
}

#[derive(Debug)]
struct RateState {
    window_start: std::time::Instant,
    count: u32,
}

/// Which Reddit host a request targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    Oauth,
    Modmail,
}

pub struct RedditClient {
    /// Primary HTTP stack (platform-native TLS when available).
    http: reqwest::Client,
    /// Secondary stack (rustls), compiled in for dual-stack failover builds.
    #[cfg(feature = "tls-failover")]
    failover_http: reqwest::Client,
    /// Set after repeated edge block pages; routes requests to `failover_http`.
    failover_active: std::sync::atomic::AtomicBool,
    cfg: Config,
    auth: tokio::sync::Mutex<AuthState>,
    rate: tokio::sync::Mutex<RateState>,
}

impl RedditClient {
    pub fn new(cfg: Config) -> Result<Self, RedditError> {
        let http = Self::primary_client(&cfg)?;
        #[cfg(feature = "tls-failover")]
        let failover_http = Self::failover_client(&cfg)?;
        let store_path =
            auth::token_store_path(&cfg).map_err(|e| RedditError::Io(e.to_string()))?;
        tracing::info!(path = %store_path.display(), "loading token store");
        Ok(Self {
            http,
            #[cfg(feature = "tls-failover")]
            failover_http,
            failover_active: std::sync::atomic::AtomicBool::new(false),
            cfg,
            auth: tokio::sync::Mutex::new(AuthState {
                store: TokenStore::load(&store_path),
                pending_state: None,
                app_only: None,
            }),
            rate: tokio::sync::Mutex::new(RateState {
                window_start: std::time::Instant::now(),
                count: 0,
            }),
        })
    }

    fn base_builder(cfg: &Config) -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .user_agent(cfg.user_agent.clone())
            .timeout(Duration::from_secs(30))
    }

    fn primary_client(cfg: &Config) -> Result<reqwest::Client, RedditError> {
        let mut builder = Self::base_builder(cfg);
        #[cfg(any(feature = "tls-failover", feature = "native-tls-backend"))]
        {
            builder = builder.use_native_tls();
        }
        #[cfg(all(
            not(any(feature = "tls-failover", feature = "native-tls-backend")),
            feature = "rustls-backend"
        ))]
        {
            builder = builder.use_rustls_tls();
        }
        builder
            .build()
            .map_err(|e| RedditError::Network(e.to_string()))
    }

    #[cfg(feature = "tls-failover")]
    fn failover_client(cfg: &Config) -> Result<reqwest::Client, RedditError> {
        Self::base_builder(cfg)
            .use_rustls_tls()
            .build()
            .map_err(|e| RedditError::Network(e.to_string()))
    }

    /// The HTTP stack requests are currently routed through.
    fn http_now(&self) -> &reqwest::Client {
        #[cfg(feature = "tls-failover")]
        if self
            .failover_active
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return &self.failover_http;
        }
        &self.http
    }

    /// One-way switch to the failover stack; true if this call flipped it.
    fn switch_to_failover(&self) -> bool {
        #[cfg(feature = "tls-failover")]
        {
            if !self
                .failover_active
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                tracing::warn!(
                    "switching HTTP stack to the rustls failover after Reddit edge block pages"
                );
                return true;
            }
            false
        }
        #[cfg(not(feature = "tls-failover"))]
        {
            false
        }
    }

    /// Whether the failover TLS stack is currently active.
    pub fn failover_active(&self) -> bool {
        self.failover_active
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn tls_stack_name(&self) -> String {
        #[cfg(feature = "tls-failover")]
        {
            if self.failover_active() {
                "rustls (failover active)".to_string()
            } else {
                "native-tls (primary)".to_string()
            }
        }
        #[cfg(all(feature = "native-tls-backend", not(feature = "tls-failover")))]
        {
            "native-tls".to_string()
        }
        #[cfg(all(
            feature = "rustls-backend",
            not(any(feature = "tls-failover", feature = "native-tls-backend"))
        ))]
        {
            "rustls".to_string()
        }
        #[cfg(not(any(
            feature = "tls-failover",
            feature = "native-tls-backend",
            feature = "rustls-backend"
        )))]
        {
            "none".to_string()
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    fn require_config(&self) -> Result<(), RedditError> {
        if self.cfg.is_configured() {
            Ok(())
        } else {
            Err(RedditError::NotConfigured)
        }
    }

    /// The `custom` preset ships no redirect URI / user agent; both must be
    /// supplied before any OAuth traffic can work.
    fn require_http_identity(&self, need_redirect: bool) -> Result<(), RedditError> {
        let mut missing = Vec::new();
        if need_redirect && self.cfg.redirect_uri.trim().is_empty() {
            missing.push("REDDIT_REDIRECT_URI");
        }
        if self.cfg.user_agent.trim().is_empty() {
            missing.push("REDDIT_USER_AGENT");
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(RedditError::Auth(format!(
                "incomplete configuration (preset 'custom'): set {} — the redirect URI must match the one registered for your app at reddit.com/prefs/apps",
                missing.join(" and ")
            )))
        }
    }

    async fn save_store(&self, st: &AuthState) -> Result<(), RedditError> {
        let path = auth::token_store_path(&self.cfg).map_err(|e| RedditError::Io(e.to_string()))?;
        st.store
            .save(&path)
            .map_err(|e| RedditError::Io(e.to_string()))
    }

    // -----------------------------------------------------------------------
    // OAuth flows
    // -----------------------------------------------------------------------

    /// Build the authorization URL the user must open in a browser.
    pub async fn login_start(&self) -> Result<models::LoginStart, RedditError> {
        self.require_config()?;
        self.require_http_identity(true)?;
        let state = auth::random_token(22);
        let url = auth::build_authorize_url(&self.cfg, &state);
        let redirect_uri = self.cfg.redirect_uri.clone();
        {
            let mut st = self.auth.lock().await;
            st.pending_state = Some(state);
            self.save_store(&st).await?;
        }
        Ok(models::LoginStart {
            authorize_url: url,
            redirect_uri: redirect_uri.clone(),
            steps: vec![
                "Open the authorize_url in a browser and log in to Reddit as the account the agent should control.".to_string(),
                "Open the browser's devtools Network panel FIRST (F12 → Network tab; Cmd+Opt+E in Firefox).".to_string(),
                "Click \"Allow\" on the consent page (you are granting every scope, including moderation).".to_string(),
                format!(
                    "Reddit will redirect to `{redirect_uri}?state=...&code=...`, but desktop browsers (especially Firefox) silently block custom URL schemes — the address bar usually never changes. Instead, click the authorize/oauth-grant request that just appeared in the Network panel and copy its `location` RESPONSE header (starts with `{redirect_uri}?`). Some browsers do show the URL in the address bar after a failed redirect — copying that works too."
                ),
                "Pass the code (or the whole location/redirect URL) to reddit_login_complete IMMEDIATELY — codes expire in well under a minute and are single-use.".to_string(),
            ],
        })
    }

    /// Exchange the pasted authorization code for permanent tokens.
    pub async fn login_complete(&self, pasted: &str) -> Result<models::Identity, RedditError> {
        self.require_config()?;
        let (state, code) = auth::extract_code(pasted).map_err(RedditError::Auth)?;
        let mut st = self.auth.lock().await;
        if let (Some(got), Some(want)) = (state.as_deref(), st.pending_state.as_deref())
            && got != want
        {
            return Err(RedditError::Auth(
                    "state mismatch: this code belongs to a different login attempt; call reddit_login_start again".into(),
                ));
        }
        let resp = self
            .token_request(&[
                ("grant_type", "authorization_code".to_string()),
                ("code", code),
                ("redirect_uri", self.cfg.redirect_uri.clone()),
            ])
            .await?;
        let Some(refresh) = resp.refresh_token.clone() else {
            return Err(RedditError::Auth(
                "Reddit issued no refresh token; ensure the authorize URL used duration=permanent (this server always does)".into(),
            ));
        };
        st.store.refresh_token = Some(refresh);
        st.store.access_token = Some(resp.access_token);
        st.store.expires_at = Some(auth::unix_now() + resp.expires_in.unwrap_or(3600).max(0) - 60);
        st.store.scopes = resp.scope.or_else(|| Some(self.cfg.scopes.clone()));
        st.store.username = None;
        st.pending_state = None;
        self.save_store(&st).await?;
        drop(st);

        let me = self.identity().await?;
        let mut st = self.auth.lock().await;
        st.store.username = Some(me.name.clone());
        self.save_store(&st).await?;
        Ok(me)
    }

    /// Revoke tokens and wipe local credentials.
    pub async fn logout(&self) -> Result<(), RedditError> {
        let mut st = self.auth.lock().await;
        let refresh = st.store.refresh_token.clone();
        let access = st.store.access_token.clone();
        for (token, hint) in [(refresh, "refresh"), (access, "access")] {
            if let Some(t) = token
                && let Err(e) = self
                    .token_request(&[("token", t), ("token_type_hint", hint.to_string())])
                    .await
            {
                // Best-effort: always clear local state below.
                tracing::warn!(error = %e, "token revocation failed (continuing with local wipe)");
            }
        }
        let device_id = st.store.device_id.clone();
        st.store = TokenStore {
            device_id,
            ..TokenStore::fresh()
        };
        st.app_only = None;
        st.pending_state = None;
        self.save_store(&st).await?;
        Ok(())
    }

    /// Fetch the logged-in identity.
    pub async fn identity(&self) -> Result<models::Identity, RedditError> {
        let v = self.get(Base::Oauth, true, "/api/v1/me", &[]).await?;
        Ok(models::identity_from(&v))
    }

    /// Configuration + token status; optionally refreshes identity if logged in.
    pub async fn status(&self) -> Result<models::AuthStatus, RedditError> {
        let st = self.auth.lock().await;
        let mut status = models::AuthStatus {
            configured: self.cfg.is_configured(),
            preset: self.cfg.preset_name.clone(),
            redirect_uri: self.cfg.redirect_uri.clone(),
            user_agent: self.cfg.user_agent.clone(),
            client_id: mask_secret(&self.cfg.client_id),
            logged_in: st.store.logged_in(),
            username: st.store.username.clone(),
            scopes: st.store.scopes.clone(),
            access_token_expires_utc: st.store.expires_at,
            identity: None,
            tls_stack: self.tls_stack_name(),
        };
        drop(st);
        if status.logged_in
            && status.configured
            && let Ok(me) = self.identity().await
        {
            status.identity = Some(me);
            // identity() may have just refreshed the token; report the new expiry.
            status.access_token_expires_utc = self.auth.lock().await.store.expires_at;
        }
        Ok(status)
    }

    async fn token_request(&self, form: &[(&str, String)]) -> Result<TokenResponse, RedditError> {
        let url = format!("{}/api/v1/access_token", self.cfg.www_base);
        let secret = self.cfg.client_secret.clone().unwrap_or_default();
        let mut attempt = 0;
        loop {
            let resp = self
                .http_now()
                .post(&url)
                .basic_auth(self.cfg.client_id.trim(), Some(secret.clone()))
                .form(form)
                .send()
                .await
                .map_err(|e| RedditError::Network(e.to_string()))?;
            let status = resp.status().as_u16();
            let body_text = resp
                .text()
                .await
                .map_err(|e| RedditError::Network(e.to_string()))?;
            // Transient edge block pages (HTML on a JSON endpoint): retry
            // once on the same stack, then switch to the failover stack.
            if status == 403 && body_text.trim_start().starts_with('<') && attempt < 2 {
                tracing::warn!(attempt, "403 block page on token endpoint, retrying");
                if !(attempt == 1 && self.switch_to_failover()) {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                attempt += 1;
                continue;
            }
            let body: Value = serde_json::from_str(&body_text).map_err(|e| {
                RedditError::Network(format!("token endpoint returned non-JSON: {e}"))
            })?;
            if !(200..300).contains(&status) {
                let err = body
                    .get("error")
                    .and_then(|e| e.as_str().map(str::to_string))
                    .or_else(|| {
                        body.get("error")
                            .and_then(|e| e.as_i64())
                            .map(|n| n.to_string())
                    })
                    .unwrap_or_else(|| status.to_string());
                let grant = form
                    .iter()
                    .find(|(k, _)| k == &"grant_type")
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("");
                let msg = match err.as_str() {
                    "invalid_grant" => {
                        "authorization code or refresh token is invalid, expired, or already used — restart the login flow (reddit_login_start)"
                    }
                    "invalid_client" => {
                        "client ID (or secret) rejected — check REDDIT_CLIENT_ID; web-app style secrets belong in REDDIT_CLIENT_SECRET"
                    }
                    "unsupported_grant_type" => "unsupported grant type (unexpected)",
                    // Reddit's edge replies with numeric-error JSON bodies
                    // ({"error": 400/404}) instead of OAuth error strings.
                    _ => match (status, grant) {
                        (400 | 404, "authorization_code") => {
                            "authorization code is invalid or expired (codes live only a few minutes) — click Allow again for a fresh code and pass it to reddit_login_complete immediately"
                        }
                        (400 | 401 | 404, "refresh_token") => {
                            "refresh token rejected (possibly revoked) — log in again via reddit_login_start"
                        }
                        (400 | 403, "https://oauth.reddit.com/grants/installed_client") => {
                            "app-only token request denied — Reddit may be blocking the configured user agent"
                        }
                        _ => "token request denied",
                    },
                };
                return Err(RedditError::Auth(format!(
                    "{msg} [HTTP {status}, error={err}]"
                )));
            }
            return serde_json::from_value(body)
                .map_err(|e| RedditError::BadShape(format!("token response: {e}")));
        }
    }

    /// Obtain a valid user access token, refreshing when needed.
    async fn user_token(&self) -> Result<String, RedditError> {
        let mut st = self.auth.lock().await;
        let Some(refresh) = st.store.refresh_token.clone() else {
            return Err(RedditError::NotLoggedIn);
        };
        let fresh = st
            .store
            .expires_at
            .map(|exp| exp - auth::unix_now() > 120)
            .unwrap_or(false);
        if !fresh {
            let resp = self
                .token_request(&[
                    ("grant_type", "refresh_token".to_string()),
                    ("refresh_token", refresh),
                ])
                .await?;
            st.store.access_token = Some(resp.access_token.clone());
            st.store.expires_at =
                Some(auth::unix_now() + resp.expires_in.unwrap_or(3600).max(0) - 60);
            if resp.scope.is_some() {
                st.store.scopes = resp.scope;
            }
            self.save_store(&st).await?;
            Ok(resp.access_token)
        } else {
            Ok(st.store.access_token.clone().unwrap_or_default())
        }
    }

    /// App-only (user-less) token for public read-only data.
    async fn app_only_token(&self) -> Result<String, RedditError> {
        self.require_http_identity(false)?;
        let mut st = self.auth.lock().await;
        if let Some((token, exp)) = &st.app_only
            && *exp - auth::unix_now() > 120
        {
            return Ok(token.clone());
        }
        let device_id = st.store.device_id.clone();
        let resp = self
            .token_request(&[
                (
                    "grant_type",
                    "https://oauth.reddit.com/grants/installed_client".to_string(),
                ),
                ("device_id", device_id),
            ])
            .await?;
        let exp = auth::unix_now() + resp.expires_in.unwrap_or(3600).max(0) - 60;
        st.app_only = Some((resp.access_token.clone(), exp));
        Ok(resp.access_token)
    }

    async fn token_for(&self, require_user: bool) -> Result<String, RedditError> {
        let logged_in = self.auth.lock().await.store.logged_in();
        if logged_in {
            self.user_token().await
        } else if require_user {
            Err(RedditError::NotLoggedIn)
        } else {
            self.app_only_token().await
        }
    }

    // -----------------------------------------------------------------------
    // Rate limiting
    // -----------------------------------------------------------------------

    async fn wait_for_slot(&self) {
        loop {
            let wait = {
                let mut r = self.rate.lock().await;
                let elapsed = r.window_start.elapsed();
                if elapsed >= Duration::from_secs(60) {
                    r.window_start = std::time::Instant::now();
                    r.count = 0;
                }
                if r.count < self.cfg.rate_limit_per_minute {
                    r.count += 1;
                    return;
                }
                Duration::from_secs(60).saturating_sub(elapsed)
            };
            tracing::debug!(?wait, "client-side rate budget reached, waiting");
            tokio::time::sleep(wait).await;
        }
    }

    // -----------------------------------------------------------------------
    // Request pipeline
    // -----------------------------------------------------------------------

    pub async fn get(
        &self,
        base: Base,
        require_user: bool,
        path: &str,
        query: &[(String, String)],
    ) -> Result<Value, RedditError> {
        self.request(base, require_user, path, query, None, true)
            .await
    }

    pub async fn post(
        &self,
        path: &str,
        form: Vec<(String, String)>,
    ) -> Result<Value, RedditError> {
        self.request(Base::Oauth, true, path, &[], Some(form), true)
            .await
    }

    /// POST to the new-modmail host (different base, no `api_type` wrapper).
    pub async fn request_modmail_post(
        &self,
        path: &str,
        form: Vec<(String, String)>,
    ) -> Result<Value, RedditError> {
        self.request(Base::Modmail, true, path, &[], Some(form), false)
            .await
    }

    /// Cached username of the logged-in account, if any.
    pub async fn current_username(&self) -> Option<String> {
        self.auth.lock().await.store.username.clone()
    }

    async fn request(
        &self,
        base: Base,
        require_user: bool,
        path: &str,
        query: &[(String, String)],
        form: Option<Vec<(String, String)>>,
        inject_api_type: bool,
    ) -> Result<Value, RedditError> {
        let url = format!(
            "{}{path}",
            match base {
                Base::Oauth => &self.cfg.oauth_base,
                Base::Modmail => &self.cfg.mod_base,
            }
        );
        let mut token = self.token_for(require_user).await?;
        let mut attempt = 0;
        loop {
            self.wait_for_slot().await;
            let mut qb: Vec<(String, String)> = query.to_vec();
            if form.is_none() {
                qb.push(("raw_json".into(), "1".into()));
            }
            let mut req = self.http_now().request(
                if form.is_some() {
                    reqwest::Method::POST
                } else {
                    reqwest::Method::GET
                },
                &url,
            );
            req = req.bearer_auth(&token).query(&qb);
            if let Some(form) = &form {
                let mut all = Vec::new();
                if inject_api_type {
                    all.push(("api_type".to_string(), "json".to_string()));
                }
                all.extend(form.iter().cloned());
                req = req.form(&all);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| RedditError::Network(e.to_string()))?;
            let status = resp.status().as_u16();

            if status == 429 {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .or_else(|| {
                        resp.headers()
                            .get("x-ratelimit-reset")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<f64>().ok())
                            .map(|x| x.ceil() as u64)
                    })
                    .unwrap_or(10);
                if attempt == 0 && retry_after <= 30 {
                    tracing::warn!(retry_after, "429 from Reddit, backing off");
                    tokio::time::sleep(Duration::from_secs(retry_after.max(1))).await;
                    attempt += 1;
                    continue;
                }
                return Err(RedditError::RateLimited(retry_after));
            }

            if status == 401 {
                let logged_in = self.auth.lock().await.store.logged_in();
                if logged_in && attempt == 0 {
                    tracing::warn!("401 from Reddit, forcing token refresh");
                    // Force a refresh by clearing the cached expiry.
                    {
                        let mut st = self.auth.lock().await;
                        st.store.expires_at = Some(0);
                    }
                    token = self.user_token().await?;
                    attempt += 1;
                    continue;
                }
            }

            let body_text = resp
                .text()
                .await
                .map_err(|e| RedditError::Network(e.to_string()))?;
            let value: Value = serde_json::from_str(&body_text)
                .unwrap_or(Value::String(body_text.chars().take(400).collect()));

            // Reddit's edge intermittently serves HTML "block pages" to
            // fingerprinted TLS clients (observed with rustls handshakes).
            // One block page is treated as transient (retry same stack); a
            // second switches to the failover TLS stack when compiled in.
            if status == 403 && body_text.trim_start().starts_with('<') {
                if attempt < 2 {
                    tracing::warn!(attempt, "403 block page from Reddit's edge, retrying");
                    if !(attempt == 1 && self.switch_to_failover()) {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    attempt += 1;
                    continue;
                }
                return Err(RedditError::Http {
                    status,
                    message: "repeated 403 block pages from Reddit's edge — this build's TLS                               fingerprint appears to be blocked; try the other TLS backend                               (see README, TLS backends)"
                        .into(),
                });
            }

            if !(200..300).contains(&status) {
                let message = value
                    .get("message")
                    .and_then(|m| m.as_str())
                    .or_else(|| value.get("error").and_then(|m| m.as_str()))
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| body_text.chars().take(200).collect());
                let mut msg = message.to_string();
                if status == 403 {
                    msg.push_str(
                        " (403 usually means: missing scope, not a moderator, or Reddit rejecting the user agent)",
                    );
                }
                return Err(RedditError::Http {
                    status,
                    message: msg,
                });
            }

            // Reddit frequently answers 200 OK with a JSON error payload.
            if let Some(errors) = value
                .pointer("/json/errors")
                .and_then(|e| e.as_array())
                .map(|a| a.as_slice())
                && !errors.is_empty()
            {
                return Err(RedditError::Api(format_api_errors(errors)));
            }
            return Ok(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_error_triples_with_hints() {
        let errors = serde_json::json!([[
            "RATELIMIT",
            "You are doing that too much. Try again in 9 minutes.",
            "ratelimit"
        ]]);
        let out = format_api_errors(errors.as_array().unwrap());
        assert!(out.contains("RATELIMIT"));
        assert!(out.contains("retry"));
    }

    #[test]
    fn handles_extra_error_fields() {
        let errors =
            serde_json::json!([["QUOTA_FILLED", "list full", "quota", {"candidate": "somesub"}]]);
        let out = format_api_errors(errors.as_array().unwrap());
        assert!(out.contains("full"));
    }
}
