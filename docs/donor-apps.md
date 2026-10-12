# Community workaround: accessibility-exempt client IDs

> **Read this first.** This page documents a community workaround, not a
> supported path. It borrows OAuth credentials from Reddit clients that hold
> accessibility exemptions (free API access). Whether you use it is between
> you and Reddit's terms of service; this project takes no position and
> receives no benefit from it.
>
> **Collective risk, stated plainly:** the App ID these apps use is shared by
> *everyone* following this workaround — it identifies the donor app, not you.
> If Reddit decides the pattern looks like third-party-client abuse (it has
> revoked such keys before, e.g. the June 2026 revocation wave that hit
> users of several third-party clients), it revokes that one ID and **everyone
> using the workaround loses access at once** — including you, until you
> repeat this process with a different donor app. Low personal volume looks
> like one more user of that app; mass adoption is what gets IDs killed.
> If your account *can* still create its own app, [Option B in the README](../README.md#setup)
> removes the shared risk entirely — most accounts currently can't, which is
> why this guide exists.

## Which donor app?

- **Dystopia (iOS)** — prefer this if you have any iOS device. Its client ID
  is not published anywhere in source, so the population borrowing it stays
  small — less exposure for a revocation sweep to notice.
- **RedReader (Android)** — use this if you're Android-only. Its client ID is
  readable in the app's open-source repository and is borrowed heavily by the
  community patching ecosystem, making it the more prominent sweep target —
  but it is equally functional for this purpose.

Both are verified working with this server's presets (Dystopia was tested
end-to-end: full `*` scope login, live reads and writes). Either ID can be
revoked at any time, and one revocation takes every borrower down together —
which is the core argument for creating your own app if you possibly can.

## Getting a donor App ID (one time)

1. Install **Dystopia** from the App Store (iOS) or **RedReader** from the
   Play Store (Android).
2. Log in with your Reddit account in the app and allow it access.
3. Reddit emails you *"You've authorized a new app in your Reddit account"*.
   Open it and copy the text after **"App ID"**.

## Configure reddit-mcp

Set the matching preset and the App ID — the preset fills the redirect URI
and user agent that must match the donor app exactly:

```toml
# ~/.config/reddit-mcp/config.toml
client_id = "THE_APP_ID_FROM_THE_EMAIL"
preset = "dystopia"            # or "redreader"
```

| Preset | Redirect URI | User agent |
|---|---|---|
| `dystopia` | `dystopia://response` | `ios:com.CarbonDev.Dystopia:v1.0.1(by /u/DystopiaForReddit)` |
| `redreader` | `redreader://rr_oauth_redir` | `RedReader/1.25.1` |

Do not modify these values to "personalize" them — the exemption rides on
the exact strings, and a mismatch breaks login (redirect URI) or invites
edge blocking (user agent).

## Login notes

Both presets use custom URL schemes as redirects, which desktop browsers
silently block — follow the devtools method in the
[main README's first-login section](../README.md#first-login): open the
Network panel, click Allow, copy the `location` response header, and paste
it to the agent immediately (codes expire in well under a minute).

## If it stops working

- `invalid_client` / HTTP 401 on token requests: the donor ID was likely
  revoked or the app was uninstalled/disconnected from your Reddit account
  (check [reddit.com/prefs/apps](https://www.reddit.com/prefs/apps) →
  "connected applications"). Repeat this guide with a fresh authorization.
- Login worked but API calls return HTML 403 pages: that is TLS-fingerprint
  blocking, not revocation — see the
  [TLS backends section](../README.md#tls-backends-and-reddits-edge-important-for-linuxbsd)
  of the README.
