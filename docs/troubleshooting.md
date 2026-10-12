# Troubleshooting

## 403 HTML block pages / TLS fingerprints

Reddit's CDN TLS-fingerprints clients (JA3/JA4-style hashes of the TLS
`ClientHello` — cipher order, extensions, curves — decided before any HTTP
header is sent, so the user agent can't fix it). As of October 2026 it
intermittently serves 403 HTML block pages to **rustls** handshakes.

The default build handles this two ways:

1. **Platform-native TLS first.** `native-tls` compiles to Security.framework
   (macOS), OpenSSL (Linux), SChannel (Windows) — the most common
   non-browser fingerprint on each platform, none observed blocked.
2. **Dual-stack failover.** Both stacks are compiled in. One block page is
   retried on the same stack; a second switches to the other stack
   (sticky) and retries. `reddit_auth_status` reports the active stack in
   `tls_stack`.

Single-stack builds for smaller/static binaries (rustls-only accepts more
block-page risk):

```sh
cargo build --release --no-default-features --features rustls-backend
cargo build --release --no-default-features --features native-tls-backend
```

If both fingerprints get blocked on your platform, the definitive fix is a
browser-impersonating HTTP client ([wreq](https://github.com/wreq) — JA4 +
HTTP/2 profiles for 100+ browsers); heavy dependency (BoringSSL, C++
toolchain), deliberately not wired in by default.

## Authorization code expired / invalid

OAuth codes are single-use and live **well under a minute** (observed October
2026). Click Allow, copy the code, paste it to the agent immediately. A dead
code fails with a clear message; restart the login flow — don't retry the
same code.

## `BAD_CAPTCHA`

Some actions (notably private messages from low-standing accounts) can
trigger CAPTCHAs. Headless clients cannot solve them; the tool error says
so. Wait and retry later or act from a more established account.

## Credential hygiene

- Name your app something generic and personal — nothing that identifies a
  third-party client or bot.
- Use a personalized user agent in Reddit's format:
  `platform:app.id:v1.0 (by /u/your_username)`.
- One ID, one user. Never publish your App ID or share it with a group:
  Reddit revokes client IDs seen authenticating from many unrelated
  machines, and a shared ID dies for everyone at once.
- Reddit has been restricting app creation and revoking third-party-tied
  keys in some regions/account classes. See [donor-apps.md](donor-apps.md)
  for the community workaround and its collective risk.

## Revocation vs. TLS blocking

- `invalid_client` / 401 on token requests → the client ID was revoked or
  the app was disconnected from your account
  ([prefs/apps](https://www.reddit.com/prefs/apps) → connected
  applications).
- Login works but API calls return HTML 403 pages → TLS fingerprint
  blocking; see above — the client fails over automatically, retry once
  after a few seconds if a single call fails.
