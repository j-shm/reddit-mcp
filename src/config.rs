//! Configuration for `reddit-mcp`.
//!
//! OAuth client configuration: a client ID plus the redirect URI and user
//! agent registered for it on Reddit. Values can come from environment
//! variables or from an optional TOML file at `~/.config/reddit-mcp/config.toml`
//! (environment variables win).

use std::path::PathBuf;

/// A named credential preset that fills known redirect URI / user agent
/// values (see docs/donor-apps.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub redirect_uri: &'static str,
    pub user_agent: &'static str,
}

pub const DYSTOPIA: Preset = Preset {
    name: "dystopia",
    redirect_uri: "dystopia://response",
    user_agent: "ios:com.CarbonDev.Dystopia:v1.0.1(by /u/DystopiaForReddit)",
};

pub const REDREADER: Preset = Preset {
    name: "redreader",
    redirect_uri: "redreader://rr_oauth_redir",
    user_agent: "RedReader/1.25.1",
};

pub const CUSTOM: Preset = Preset {
    name: "custom",
    redirect_uri: "",
    user_agent: "",
};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    Toml {
        path: PathBuf,
        source: toml::de::Error,
    },
}

/// Resolved runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// OAuth client ID (Reddit "App ID"). May be empty; the server starts and
    /// reports itself as unconfigured instead of failing.
    pub client_id: String,
    /// Optional client secret (only for confidential "web app" / "script"
    /// apps; donor installed-app credentials leave this empty).
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub user_agent: String,
    /// OAuth scopes to request at login. Defaults to `*` (every scope).
    pub scopes: String,
    /// Which preset supplied the redirect URI / user agent defaults.
    pub preset_name: String,
    /// Client-side requests-per-minute budget (Reddit allows ~100 QPM per
    /// OAuth client; stay under it).
    pub rate_limit_per_minute: u32,
    // Base URLs, overridable for tests.
    pub www_base: String,
    pub oauth_base: String,
    pub mod_base: String,
    /// Override for the credential/data directory (tests).
    pub data_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: None,
            redirect_uri: DYSTOPIA.redirect_uri.to_string(),
            user_agent: DYSTOPIA.user_agent.to_string(),
            scopes: "*".to_string(),
            preset_name: DYSTOPIA.name.to_string(),
            rate_limit_per_minute: 50,
            www_base: "https://www.reddit.com".to_string(),
            oauth_base: "https://oauth.reddit.com".to_string(),
            mod_base: "https://mod.reddit.com".to_string(),
            data_dir: None,
        }
    }
}

impl Config {
    pub fn is_configured(&self) -> bool {
        !self.client_id.trim().is_empty()
    }

    /// Load configuration: defaults <- TOML file <- environment variables.
    pub fn load() -> Result<Self, ConfigError> {
        let mut cfg = Config::default();
        if let Some(path) = config_file_path()
            && path.exists()
        {
            let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
                path: path.clone(),
                source,
            })?;
            let table: toml::Table = toml::from_str(&text).map_err(|source| ConfigError::Toml {
                path: path.clone(),
                source,
            })?;
            if let Some(v) = str_at(&table, "client_id") {
                cfg.client_id = v;
            }
            if let Some(v) = str_at(&table, "client_secret") {
                cfg.client_secret = Some(v);
            }
            if let Some(v) = str_at(&table, "redirect_uri") {
                cfg.redirect_uri = v;
            }
            if let Some(v) = str_at(&table, "user_agent") {
                cfg.user_agent = v;
            }
            if let Some(v) = str_at(&table, "preset") {
                apply_preset(&mut cfg, &v);
            }
            if let Some(v) = str_at(&table, "scopes") {
                cfg.scopes = v;
            }
            if let Some(v) = table
                .get("rate_limit_per_minute")
                .and_then(|v| v.as_integer())
                && v > 0
            {
                cfg.rate_limit_per_minute = v as u32;
            }
        }

        if let Ok(v) = std::env::var("REDDIT_CLIENT_ID") {
            cfg.client_id = v;
        }
        if let Ok(v) = std::env::var("REDDIT_CLIENT_SECRET") {
            cfg.client_secret = Some(v);
        }
        if let Ok(v) = std::env::var("REDDIT_PRESET") {
            apply_preset(&mut cfg, &v);
        }
        if let Ok(v) = std::env::var("REDDIT_REDIRECT_URI") {
            cfg.redirect_uri = v;
        }
        if let Ok(v) = std::env::var("REDDIT_USER_AGENT") {
            cfg.user_agent = v;
        }
        if let Ok(v) = std::env::var("REDDIT_SCOPES") {
            cfg.scopes = v;
        }
        if let Ok(v) = std::env::var("REDDIT_RATE_LIMIT")
            && let Ok(n) = v.parse::<u32>()
            && n > 0
        {
            cfg.rate_limit_per_minute = n;
        }
        Ok(cfg)
    }
}

fn str_at(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn apply_preset(cfg: &mut Config, preset: &str) {
    let preset = preset.trim().to_ascii_lowercase();
    let p = match preset.as_str() {
        "dystopia" => &DYSTOPIA,
        "redreader" => &REDREADER,
        "custom" => &CUSTOM,
        _ => {
            tracing::warn!(preset, "unknown REDDIT_PRESET, keeping current values");
            return;
        }
    };
    cfg.preset_name = p.name.to_string();
    if !p.redirect_uri.is_empty() {
        cfg.redirect_uri = p.redirect_uri.to_string();
    }
    if !p.user_agent.is_empty() {
        cfg.user_agent = p.user_agent.to_string();
    }
}

/// Path of the optional TOML config file.
/// Base configuration directory. Prefers the XDG-style `~/.config` on Unix
/// (the documented location) and falls back to the platform config dir.
fn base_config_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home).join(".config"));
    }
    dirs::config_dir()
}

pub fn config_file_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("REDDIT_CONFIG_PATH")
        && !p.trim().is_empty()
    {
        return Some(PathBuf::from(p));
    }
    base_config_dir().map(|d| d.join("reddit-mcp").join("config.toml"))
}

/// Directory holding the config file and token store; created on demand.
pub fn data_dir() -> Result<PathBuf, std::io::Error> {
    let dir = match std::env::var("REDDIT_DATA_DIR") {
        Ok(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => base_config_dir()
            .map(|d| d.join("reddit-mcp"))
            .ok_or_else(|| {
                std::io::Error::other(
                    "cannot determine a configuration directory for this platform",
                )
            })?,
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Mask a client ID for display (never log or return full credentials).
pub fn mask_secret(s: &str) -> String {
    let n = s.chars().count();
    if n == 0 {
        "<not set>".to_string()
    } else if n <= 8 {
        format!("{}…", s.chars().take(2).collect::<String>())
    } else {
        format!(
            "{}…{}",
            s.chars().take(6).collect::<String>(),
            s.chars().rev().take(2).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_dystopia_preset() {
        let c = Config::default();
        assert_eq!(c.preset_name, "dystopia");
        assert!(!c.redirect_uri.is_empty());
        assert!(!c.user_agent.is_empty());
    }

    #[test]
    fn presets_fill_known_values() {
        let mut c = Config::default();
        apply_preset(&mut c, "dystopia");
        assert_eq!(c.redirect_uri, "dystopia://response");
        assert!(!c.user_agent.is_empty());
        apply_preset(&mut c, "RedReader");
        assert_eq!(c.redirect_uri, "redreader://rr_oauth_redir");
        assert_eq!(c.user_agent, "RedReader/1.25.1");
        // explicit overrides survive a no-op preset
        apply_preset(&mut c, "unknown-preset");
        assert_eq!(c.redirect_uri, "redreader://rr_oauth_redir");
    }
}
