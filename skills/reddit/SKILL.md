---
name: reddit
description: Browse, read, search, post, comment, vote, message, and moderate Reddit through the reddit MCP server. Use whenever the user mentions Reddit, a subreddit (r/...), a post or comment link, their inbox or karma, or asks to post/reply/vote/moderate anything — even casual phrasing like "check reddit", "what's on r/rust", or "reply to that thread".
---

# Using the reddit MCP server

You have 25 `reddit_*` tools. Reading works without login (public content,
app-only access); everything else — posting, voting, messaging, moderation —
needs a completed login, which only ever happens once (tokens are permanent
until `reddit_logout`).

Always start with `reddit_auth_status` the first time Reddit comes up in a
session. It tells you whether the server is configured, logged in, and whose
account (`username`) you are acting as. Quote that username back to the user
before doing anything destructive.

## The login flow (rare, but you must drive it exactly)

You cannot log in yourself — the user must authorize in their browser. The
flow has strict timing, so prepare the user before they start:

1. Call `reddit_login_start`. You get an `authorize_url` and instructions.
2. Tell the user, in one message, everything they need:
   - open the URL and log in as the account you should control
   - **open the devtools Network panel first** (F12 → Network; Cmd+Opt+E in
     Firefox) — desktop browsers silently block the redirect, so the address
     bar usually never changes
   - click **Allow**, then click the authorize/`oauth-grant` request in the
     Network panel and copy its `location` response header
3. The moment they paste it back, call `reddit_login_complete` with the paste
   as `input` (a bare code, a full redirect URL, or a devtools copy with the
   "location" label all parse fine).

The authorization code expires in **well under a minute** and is single-use.
If exchange fails with "code is invalid or expired", apologize once, then
restart the flow — do not retry the dead code.

## Core conventions

- **Fullnames**: posts are `t3_...`, comments `t1_...`, messages `t4_...`.
  Every result carries them in `id` / `post_id` / `parent_id`. Comment on a
  post with `parent_id: "t3_..."`; reply to a comment with `"t1_..."`.
- **Pagination**: every listing returns `next_after` (an opaque cursor). To
  fetch the next page, pass it back as `after`. Never modify it; stop when
  it's absent.
- **Truncation**: long bodies/selftext are cut around 2000 chars and marked
  `truncated: true`. If you need the rest, fetch the post's `permalink` in a
  browser tool or say so — don't pretend you read the full text.
- **Errors are text, and they're advice**: tool failures arrive as normal
  `isError` results with actionable messages (`RATELIMIT` → wait the stated
  time and retry once; `ALREADY_SUB` → retry with `resubmit: true`;
  `THREAD_LOCKED`, `NOT_AUTHOR`, `USER_BANNED_FROM_SUBREDDIT` → explain to
  the user). Only retry when the message says to.
- **Write actions trigger client-side confirmation** (they're flagged
  destructive). If the user declines, stop — never silently retry.

## Choosing tools

| User wants | Tool |
|---|---|
| Front page / subreddit / multi feed | `reddit_get_feed` (`sort`, `time`, `after`) |
| A post + its comments | `reddit_get_post` (flattened tree, `depth` per comment) |
| Search posts/subs/users | `reddit_search` |
| A user's profile/history | `reddit_get_user` (`section`) |
| Subreddit info + rules | `reddit_get_subreddit` |
| Inbox, unread, mentions | `reddit_get_inbox` |
| Subscriptions list | `reddit_list_subreddits` |
| Create post / comment | `reddit_create_post`, `reddit_create_comment` |
| Edit or delete your content | `reddit_edit_or_delete` |
| Vote / save / hide / report | `reddit_item_action` |
| Flair | `reddit_set_flair` — call it **without** a template id first to list the available options |
| Subscribe/unsubscribe | `reddit_subscribe` |
| Private messages | `reddit_send_message`, `reddit_mark_read` |
| Mod queue / log | `reddit_mod_queue`, `reddit_mod_log` |
| Remove, approve, lock, sticky, distinguish, NSFW, spoiler | `reddit_mod_action` |
| Ban / mute / mod invite / contributor | `reddit_mod_user` |
| Who's banned/muted/moderating | `reddit_mod_users` |
| New modmail | `reddit_modmail` (list / get / reply) |

## Workflows worth internalizing

**Posting**: before submitting to a subreddit for the first time, call
`reddit_get_subreddit` and read the rules back to the user. Draft the title
and body, show them, then submit after confirmation. Pass `flair_id` only if
the sub requires it (list options via `reddit_set_flair`).

**Comment threads**: when the user says "reply to that", identify what they
linked: a post → `t3_` top-level comment; a comment → `t1_` reply. Show the
draft before calling `reddit_create_comment`.

**Inbox triage**: `reddit_get_inbox` → summarize unread; for each item
propose a reply or an action; batch confirmations; `reddit_mark_read` at the
end (ids or `all: true`).

**Moderation**: `reddit_mod_queue` → for each item show
`report_reasons` and the content; then one `reddit_mod_action` per decision.
Bans take `duration_days` (omit = permanent), a private `note`, and a
`message_to_user` shown to the banned person — draft both and confirm.

## Hard limits to respect

- **CAPTCHAs** (`BAD_CAPTCHA`) cannot be solved headlessly. Tell the user and
  stop; retrying hammers the same wall.
- Mod tools fail with a clear 403-flavored error if the account isn't a
  moderator of that subreddit — surface it, don't work around it.
- Don't parallel-fire the same mutation; the server rate-limits itself
  (~50 req/min budget) and backs off on 429s — queue actions sequentially
  when acting on many items.
- If everything suddenly returns HTML-403 noise, mention that
  `reddit_auth_status` shows a `tls_stack` field and the README documents
  TLS backends — but this self-heals via automatic failover; just retry the
  call once after a few seconds.

## Credentials: which path to set up

Most users can no longer create Reddit apps at reddit.com/prefs/apps, so
**start with the donor-app setup** in `docs/donor-apps.md`:
**Dystopia** if they have any iOS device (recommended — its client ID is
unpublished in source, so fewer people borrow it), **RedReader** for
Android-only users. Config is just `client_id` + the matching preset.
Only if the user already has their own Reddit app should they use
`preset = "custom"` with their own redirect URI and user agent. Mention the
collective-revocation risk once when they pick a donor app, then move on.

## Installation

This skill accompanies the [reddit-mcp](https://github.com/j-shm/reddit-mcp)
server. The MCP client must have the server registered (stdio) and, for
anything beyond public reads, the one-time login completed. See the repo's
README for setup and `docs/donor-apps.md` for credential background.
