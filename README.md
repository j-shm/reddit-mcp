# reddit-mcp

MCP server (Rust, stdio) that gives an AI agent full Reddit control: feeds, search, posts and comment threads, commenting, voting, private messages, flair, subscriptions, and complete moderation (queues, mod log, remove/approve/lock/sticky, bans, modmail). 25 tools, MIT.

## Install

- Download a prebuilt binary from [Releases](https://github.com/j-shm/reddit-mcp/releases) (linux amd64, macOS arm64, Windows x64), or
- `cargo install reddit-mcp` (once published to crates.io), or build from source: `cargo build --release`.

## Setup

**Option A — donor App ID** (works for almost everyone; Reddit blocks new app creation for most accounts): follow the two-minute guide in [docs/donor-apps.md](docs/donor-apps.md) — **Dystopia recommended** — then:

```toml
# ~/.config/reddit-mcp/config.toml
client_id = "THE_APP_ID"
preset = "dystopia"            # or "redreader"
```

**Option B — your own app**, if your account can still create one at [reddit.com/prefs/apps](https://www.reddit.com/prefs/apps) (installed app, redirect `http://localhost:8080`):

```toml
client_id = "YOUR_APP_ID"
preset = "custom"
redirect_uri = "http://localhost:8080"
user_agent = "macos:com.example.myredditmcp:v1.0 (by /u/your_username)"
```

Then register with your MCP client:

```json
{ "mcpServers": { "reddit": { "command": "/path/to/reddit-mcp" } } }
```

Env vars (`REDDIT_CLIENT_ID`, `REDDIT_PRESET`, `REDDIT_REDIRECT_URI`, `REDDIT_USER_AGENT`, `REDDIT_SCOPES`, `REDDIT_RATE_LIMIT`, `REDDIT_CLIENT_SECRET`) override the TOML. Tokens live in `~/.config/reddit-mcp/tokens.json` (0600, never logged).

## First login

Tell your agent to log in to Reddit. It hands you an authorize URL; log in, click **Allow**, and paste the redirect back to the agent **immediately** — codes expire in under a minute. With a `localhost` redirect the code is right in the address bar; for custom URL schemes, copy the `location` response header in the devtools Network panel. The refresh token is permanent until `reddit_logout`. Reading works before login; everything else requires it.

## Tools

| Area | Tools |
|---|---|
| Auth | `reddit_login_start`, `reddit_login_complete`, `reddit_auth_status`, `reddit_logout` |
| Read | `reddit_get_feed`, `reddit_get_post` (+comment tree), `reddit_search`, `reddit_get_user`, `reddit_get_subreddit` (+rules), `reddit_get_inbox`, `reddit_list_subreddits` |
| Write | `reddit_create_post`, `reddit_create_comment`, `reddit_edit_or_delete`, `reddit_item_action` (vote/save/hide/report), `reddit_set_flair`, `reddit_subscribe`, `reddit_send_message`, `reddit_mark_read` |
| Moderation | `reddit_mod_queue`, `reddit_mod_log`, `reddit_mod_action`, `reddit_mod_user`, `reddit_mod_users`, `reddit_modmail` |

Listings paginate with the opaque `next_after` cursor; long bodies truncate (~2000 chars, marked); errors return actionable text (`ALREADY_SUB` → `resubmit=true`, `RATELIMIT` → wait and retry).

## Extras

- **Agent skill**: [`skills/reddit/SKILL.md`](skills/reddit/SKILL.md) teaches any [Agent Skills](https://agentskills.io)-compatible agent to drive the server well (login walkthrough, tool selection, conventions). Copy it into your agent's skills directory.
- **Troubleshooting**: [docs/troubleshooting.md](docs/troubleshooting.md) — TLS backends, 403 block pages, CAPTCHAs, credential hygiene.

## Development

```sh
cargo build
cargo test     # unit + wiremock integration + stdio smoke — no network or credentials needed
cargo clippy
```

MIT — see [LICENSE](LICENSE).
