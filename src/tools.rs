//! The MCP tool surface: 25 tools covering authentication, reading, writing,
//! messaging, and moderation, following MCP authoring best practices
//! (typed parameters with per-field descriptions, opaque `after` cursors,
//! truncated payloads, tool-execution errors as `isError` results, stderr-only
//! logging, destructive-hint annotations for client-side confirmation).

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::Value;

use crate::client::{Base, RedditClient, RedditError};
use crate::config::{Config, mask_secret};
use crate::models::{self, FlairOption};

#[derive(Clone)]
pub struct RedditServer {
    client: std::sync::Arc<RedditClient>,
    tool_router: ToolRouter<RedditServer>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ok<T: serde::Serialize>(value: &T, human: String) -> Result<CallToolResult, McpError> {
    let structured = serde_json::to_value(value)
        .map_err(|e| McpError::internal_error(format!("failed to serialize result: {e}"), None))?;
    let mut r = CallToolResult::success(vec![ContentBlock::text(human)]);
    r.structured_content = Some(structured);
    Ok(r)
}

fn tool_error(e: RedditError) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        e.to_string(),
    )]))
}

fn invalid(msg: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(msg.into())]))
}

fn first_thing(v: &Value) -> Option<Value> {
    v.pointer("/json/data/things/0/data").cloned()
}

fn clean_sub(input: &str) -> String {
    input
        .trim()
        .trim_start_matches('/')
        .trim_start_matches("r/")
        .trim()
        .to_string()
}

fn clean_user(input: &str) -> String {
    input
        .trim()
        .trim_start_matches('/')
        .trim_start_matches('u')
        .trim_start_matches('/')
        .trim()
        .to_string()
}

fn clamp_limit(limit: Option<u32>, max: u32) -> (String, String) {
    let n = limit.unwrap_or(25).clamp(1, max);
    ("limit".into(), n.to_string())
}

// ---------------------------------------------------------------------------
// Shared parameter types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeedSort {
    /// Sort by current hotness (default).
    Hot,
    /// Newest first.
    New,
    /// Highest scoring within the chosen time range.
    Top,
    /// Rising posts.
    Rising,
    /// Most controversial within the chosen time range.
    Controversial,
    /// Front page "best" (front-page source only).
    Best,
}

impl FeedSort {
    fn path(&self) -> &'static str {
        match self {
            FeedSort::Hot => "hot",
            FeedSort::New => "new",
            FeedSort::Top => "top",
            FeedSort::Rising => "rising",
            FeedSort::Controversial => "controversial",
            FeedSort::Best => "best",
        }
    }
    fn uses_time(&self) -> bool {
        matches!(self, FeedSort::Top | FeedSort::Controversial)
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeRange {
    Hour,
    Day,
    Week,
    Month,
    Year,
    All,
}

impl TimeRange {
    fn value(&self) -> &'static str {
        match self {
            TimeRange::Hour => "hour",
            TimeRange::Day => "day",
            TimeRange::Week => "week",
            TimeRange::Month => "month",
            TimeRange::Year => "year",
            TimeRange::All => "all",
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommentSort {
    /// Reddit's ranked sort (default).
    Best,
    Top,
    New,
    Controversial,
    Old,
    Random,
    /// Question-and-answer style.
    Qa,
}

impl CommentSort {
    fn value(&self) -> &'static str {
        match self {
            CommentSort::Best => "confidence",
            CommentSort::Top => "top",
            CommentSort::New => "new",
            CommentSort::Controversial => "controversial",
            CommentSort::Old => "old",
            CommentSort::Random => "random",
            CommentSort::Qa => "qa",
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UserSection {
    /// Profile metadata only.
    About,
    /// Posts and comments combined.
    Overview,
    Submitted,
    Comments,
    Upvoted,
    Downvoted,
    Saved,
    Hidden,
    Gilded,
}

impl UserSection {
    fn path(&self) -> &'static str {
        match self {
            UserSection::About => "about",
            UserSection::Overview => "overview",
            UserSection::Submitted => "submitted",
            UserSection::Comments => "comments",
            UserSection::Upvoted => "upvoted",
            UserSection::Downvoted => "downvoted",
            UserSection::Saved => "saved",
            UserSection::Hidden => "hidden",
            UserSection::Gilded => "gilded",
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InboxBox {
    Inbox,
    Unread,
    Sent,
    Messages,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubredditListWhere {
    Subscriber,
    Moderator,
    Contributor,
    Popular,
    New,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PostKind {
    /// Text (self) post; body goes in `text`.
    Text,
    /// Link post; target goes in `url`.
    Link,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditOrDeleteAction {
    Edit,
    Delete,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemAction {
    /// Cast or change a vote (requires `direction`).
    Vote,
    Save,
    Unsave,
    Hide,
    Unhide,
    /// Report to the moderators/admins (requires `reason` or `site_reason`).
    Report,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VoteDirection {
    Up,
    Down,
    /// Remove an existing vote.
    Clear,
}

impl VoteDirection {
    fn value(&self) -> &'static str {
        match self {
            VoteDirection::Up => "1",
            VoteDirection::Down => "-1",
            VoteDirection::Clear => "0",
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscribeAction {
    Subscribe,
    Unsubscribe,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModQueueLocation {
    /// Items flagged by users or filters awaiting mod action.
    Modqueue,
    /// User reports.
    Reports,
    /// Items removed as spam.
    Spam,
    /// Unmoderated submissions.
    Unmoderated,
    /// Recently edited items.
    Edited,
}

impl ModQueueLocation {
    fn path(&self) -> &'static str {
        match self {
            ModQueueLocation::Modqueue => "modqueue",
            ModQueueLocation::Reports => "reports",
            ModQueueLocation::Spam => "spam",
            ModQueueLocation::Unmoderated => "unmoderated",
            ModQueueLocation::Edited => "edited",
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModActionKind {
    /// Remove as a normal removal (not spam).
    Remove,
    /// Remove and flag as spam.
    Spam,
    Approve,
    /// Green sticky mod distinction on a top-level comment or post.
    Distinguish,
    /// Remove the mod distinction.
    Undistinguish,
    Sticky,
    Unsticky,
    Lock,
    Unlock,
    /// Hide pending reports on the item.
    IgnoreReports,
    UnignoreReports,
    MarkNsfw,
    UnmarkNsfw,
    MarkSpoiler,
    UnmarkSpoiler,
    /// Set the default comment sort (requires `suggested_sort`).
    SuggestedSort,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DistinguishHow {
    /// Regular moderator distinction (green).
    Yes,
    /// Admin distinction (red) — admin accounts only.
    Admin,
    /// Special distinction (gold).
    Special,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModRole {
    Banned,
    Muted,
    /// Adding a moderator sends an invite the user must accept.
    Moderator,
    ModeratorInvite,
    Contributor,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddRemove {
    Add,
    Remove,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModUsersWhere {
    Banned,
    Muted,
    Moderators,
    Contributors,
}

impl ModUsersWhere {
    fn path(&self) -> &'static str {
        match self {
            ModUsersWhere::Banned => "banned",
            ModUsersWhere::Muted => "muted",
            ModUsersWhere::Moderators => "moderators",
            ModUsersWhere::Contributors => "contributors",
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModmailAction {
    List,
    Get,
    Reply,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModmailState {
    All,
    New,
    Inprogress,
    Mod,
    Notifications,
    Archived,
    Appeals,
    Communities,
    Engagements,
    Incoming,
}

impl ModmailState {
    fn value(&self) -> &'static str {
        match self {
            ModmailState::All => "all",
            ModmailState::New => "new",
            ModmailState::Inprogress => "inprogress",
            ModmailState::Mod => "mod",
            ModmailState::Notifications => "notifications",
            ModmailState::Archived => "archived",
            ModmailState::Appeals => "appeals",
            ModmailState::Communities => "communities",
            ModmailState::Engagements => "engagements",
            ModmailState::Incoming => "incoming",
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModmailSort {
    Recent,
    Mod,
    Unread,
    User,
}

impl ModmailSort {
    fn value(&self) -> &'static str {
        match self {
            ModmailSort::Recent => "recent",
            ModmailSort::Mod => "mod",
            ModmailSort::Unread => "unread",
            ModmailSort::User => "user",
        }
    }
}

// ---------------------------------------------------------------------------
// Tool parameter structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct LoginCompleteParams {
    /// Whatever the user copied after authorizing: the bare `code` value or
    /// the entire redirect URL (e.g. `dystopia://response?state=..&code=..`).
    pub input: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct GetFeedParams {
    /// Subreddit name (with or without `r/`). Omit for the front page.
    pub subreddit: Option<String>,
    /// Name of a multireddit owned by the logged-in user.
    pub multi: Option<String>,
    /// Listing sort (default: hot).
    pub sort: Option<FeedSort>,
    /// Time range for `top` / `controversial`.
    pub time: Option<TimeRange>,
    /// Max posts to return, 1–100 (default 25).
    pub limit: Option<u32>,
    /// Opaque pagination cursor from a previous response's `next_after`.
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct GetPostParams {
    /// Post fullname (`t3_...`) or bare base36 id.
    pub post_id: String,
    /// Max top-level comment fetch depth hint (default 25).
    pub comment_limit: Option<u32>,
    /// Comment sort (default: best).
    pub comment_sort: Option<CommentSort>,
    /// Max comment tree depth.
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// Search query; supports operators like `author:`, `selftext:`, `site:`,
    /// `nsfw:yes|no`, `self:yes|no`.
    pub query: String,
    /// Restrict the search to this subreddit.
    pub subreddit: Option<String>,
    /// Relevance, hot, top, new, or comments.
    pub sort: Option<String>,
    pub time: Option<TimeRange>,
    /// Result type filter: `link`, `sr`, `user`, or comma-combined.
    pub result_type: Option<String>,
    /// Max results, 1–100 (default 25).
    pub limit: Option<u32>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct GetUserParams {
    /// Reddit username (with or without `u/`).
    pub username: String,
    /// Which part of the profile to list (default: overview).
    pub section: Option<UserSection>,
    pub time: Option<TimeRange>,
    /// Max items, 1–100 (default 25).
    pub limit: Option<u32>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct GetSubredditParams {
    /// Subreddit name (with or without `r/`).
    pub subreddit: String,
    /// Include the subreddit rules (default true).
    pub include_rules: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct GetInboxParams {
    /// Which box (default: inbox).
    pub box_: Option<InboxBox>,
    /// Mark fetched messages as read (default false).
    pub mark_read: Option<bool>,
    /// Max items, 1–100 (default 25).
    pub limit: Option<u32>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ListSubredditsParams {
    /// Which list (default: subscriber).
    pub where_: Option<SubredditListWhere>,
    /// When true, lists the logged-in user's subreddits; `popular`/`new`
    /// ignore this and list site-wide (default true).
    pub mine: Option<bool>,
    /// Max items, 1–100 (default 25).
    pub limit: Option<u32>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CreatePostParams {
    /// Target subreddit name (no `r/` needed).
    pub subreddit: String,
    /// Post title (max 300 chars).
    pub title: String,
    /// `self` (text) or `link`.
    pub kind: PostKind,
    /// Body text for self posts.
    pub text: Option<String>,
    /// Target URL for link posts.
    pub url: Option<String>,
    /// Flair template id from the subreddit's link flair options.
    pub flair_id: Option<String>,
    /// Allow reposting a URL that was already submitted (default false).
    pub resubmit: Option<bool>,
    /// Send inbox replies for the new post (default true).
    pub sendreplies: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct CreateCommentParams {
    /// Parent fullname: `t3_...` for a top-level comment on a post, `t1_...`
    /// for a reply to a comment.
    pub parent_id: String,
    /// Comment body as Markdown (max 10,000 chars).
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct EditOrDeleteParams {
    /// Fullname of the comment (`t1_`) or self post (`t3_`) owned by the
    /// logged-in user.
    pub thing_id: String,
    /// Edit (requires `text`) or delete.
    pub action: EditOrDeleteAction,
    /// New Markdown text for edits.
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ItemActionParams {
    /// Target fullname (t1_ or t3_).
    pub thing_id: String,
    pub action: ItemAction,
    /// Vote direction (for `vote`).
    pub direction: Option<VoteDirection>,
    /// Save category (for `save`).
    pub category: Option<String>,
    /// Free-text report reason, 0–100 chars (for `report`).
    pub reason: Option<String>,
    /// Reddit site rule key for `report`, e.g. `Spam`, `Harassment`.
    pub site_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SetFlairParams {
    /// Subreddit name.
    pub subreddit: String,
    /// Post fullname (t3_) to set link flair on. Omit for user flair.
    pub post_id: Option<String>,
    /// User flair target (default: the logged-in user; moderators can set
    /// other users' flair).
    pub username: Option<String>,
    /// Flair template id. Omit both this and `text` to list the available
    /// options instead.
    pub flair_template_id: Option<String>,
    /// Custom text for editable flairs.
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SubscribeParams {
    /// Subreddit names (no `r/` needed).
    pub subreddits: Vec<String>,
    pub action: SubscribeAction,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct SendMessageParams {
    /// Recipient username (or `/u/name`).
    pub to: String,
    /// Subject, max 100 chars.
    pub subject: String,
    /// Body as Markdown.
    pub body: String,
    /// Send as a subreddit you moderate (modmail-style PM).
    pub from_subreddit: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct MarkReadParams {
    /// Message fullnames (t4_... or comment-reply fullnames).
    pub message_ids: Option<Vec<String>>,
    /// Mark the entire inbox read.
    pub all: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ModQueueParams {
    pub subreddit: String,
    /// Which queue (default: modqueue).
    pub location: Option<ModQueueLocation>,
    /// Only `links` or only `comments`.
    pub only: Option<String>,
    /// Max items, 1–100 (default 25).
    pub limit: Option<u32>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ModLogParams {
    pub subreddit: String,
    /// Filter by action type, e.g. `banuser`, `removelink`, `distinguish`.
    pub action_type: Option<String>,
    /// Filter by moderator username.
    pub mod_: Option<String>,
    /// Max entries, 1–500 (default 50).
    pub limit: Option<u32>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ModActionParams {
    /// Target fullname (t1_ or t3_).
    pub thing_id: String,
    pub action: ModActionKind,
    /// Distinction style for `distinguish`.
    pub distinguish_how: Option<DistinguishHow>,
    /// Also make a distinguished top-level comment sticky.
    pub sticky: Option<bool>,
    /// Default comment sort for `suggested_sort`.
    pub suggested_sort: Option<CommentSort>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ModUserParams {
    pub subreddit: String,
    /// Target username.
    pub username: String,
    pub role: ModRole,
    pub action: AddRemove,
    /// Ban duration in days (1–999); omit or 0 for permanent (bans only).
    pub duration_days: Option<u32>,
    /// Short private-ish reason (≤100 chars).
    pub reason: Option<String>,
    /// Mod-only note (≤300 chars).
    pub note: Option<String>,
    /// Message shown to the banned user (≤1000 chars).
    pub message_to_user: Option<String>,
    /// Moderator permissions, e.g. `+all` or `+posts,+mail` (mod invites).
    pub permissions: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ModUsersParams {
    pub subreddit: String,
    pub role: ModUsersWhere,
    /// Filter to a specific username.
    pub username_filter: Option<String>,
    /// Max entries, 1–100 (default 25).
    pub limit: Option<u32>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ModmailParams {
    pub action: ModmailAction,
    /// Conversation id (for get/reply).
    pub conversation_id: Option<String>,
    /// Filter by conversation state (list).
    pub state: Option<ModmailState>,
    /// Sort for listing (default recent).
    pub sort: Option<ModmailSort>,
    /// Filter listing by subreddit names, usernames, or conversation ids.
    pub entity: Option<Vec<String>>,
    /// Max conversations for list, 1–100 (default 25).
    pub limit: Option<u32>,
    /// Reply body as Markdown (for reply).
    pub body: Option<String>,
    /// Reply on the internal mod-only thread.
    pub is_internal: Option<bool>,
    /// Hide moderator identity from the user.
    pub hide_mod_identity: Option<bool>,
    /// Mark the conversation read when fetching (default true).
    pub mark_read: Option<bool>,
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[tool_router]
impl RedditServer {
    pub fn new(config: Config) -> Result<Self, RedditError> {
        Ok(Self {
            client: std::sync::Arc::new(RedditClient::new(config)?),
            tool_router: Self::tool_router(),
        })
    }

    /// Start the Reddit OAuth login. Returns the authorization URL for the
    /// user to open in a browser (log in, click Allow) plus step-by-step
    /// instructions. The user should open the devtools Network panel before
    /// clicking Allow, then copy the `location` response header of the
    /// authorize request (desktop browsers silently block the custom-scheme
    /// redirect, so the address bar usually never changes) and pass it to
    /// reddit_login_complete immediately — codes expire in under a minute.
    #[tool(annotations(title = "Start Reddit login"))]
    async fn reddit_login_start(&self) -> Result<CallToolResult, McpError> {
        match self.client.login_start().await {
            Ok(start) => {
                let human = format!(
                    "Open this URL in a browser and authorize:\n{}\nThen pass the code (or the whole redirect URL) to reddit_login_complete.",
                    start.authorize_url
                );
                ok(&start, human)
            }
            Err(e) => tool_error(e),
        }
    }

    /// Finish the Reddit OAuth login with the code the user copied after
    /// authorizing in the browser. Accepts the bare code or the full redirect
    /// URL. On success, stores permanent credentials and returns the account
    /// identity.
    #[tool(annotations(title = "Complete Reddit login"))]
    async fn reddit_login_complete(
        &self,
        Parameters(LoginCompleteParams { input }): Parameters<LoginCompleteParams>,
    ) -> Result<CallToolResult, McpError> {
        match self.client.login_complete(&input).await {
            Ok(me) => ok(
                &me,
                format!(
                    "Logged in as u/{} ({} link karma, {} comment karma).",
                    me.name,
                    me.link_karma.unwrap_or(0),
                    me.comment_karma.unwrap_or(0)
                ),
            ),
            Err(e) => tool_error(e),
        }
    }

    /// Show configuration and login state: preset, redirect URI, user agent,
    /// masked client ID, logged-in username, granted scopes, and token expiry.
    /// Does not expose any secret.
    #[tool(annotations(title = "Reddit auth status", read_only_hint = true))]
    async fn reddit_auth_status(&self) -> Result<CallToolResult, McpError> {
        match self.client.status().await {
            Ok(status) => {
                let human = if !status.configured {
                    format!(
                        "Not configured. Set REDDIT_CLIENT_ID (preset: {}), then restart the server and run reddit_login_start.",
                        status.preset
                    )
                } else if status.logged_in {
                    format!(
                        "Configured (preset: {}, client: {}). Logged in as u/{} with scopes [{}].",
                        status.preset,
                        mask_secret(&self.client.config().client_id),
                        status.username.as_deref().unwrap_or("?"),
                        status.scopes.as_deref().unwrap_or("?")
                    )
                } else {
                    "Configured but not logged in — run reddit_login_start, then reddit_login_complete. Read-only browsing works via app-only access.".to_string()
                };
                ok(&status, human)
            }
            Err(e) => tool_error(e),
        }
    }

    /// Revoke the stored Reddit tokens and wipe local credentials. Read-only
    /// app-only browsing still works afterwards until a new login.
    #[tool(annotations(title = "Log out of Reddit", destructive_hint = true))]
    async fn reddit_logout(&self) -> Result<CallToolResult, McpError> {
        match self.client.logout().await {
            Ok(()) => ok(
                &models::ActionResult {
                    ok: true,
                    detail: "Tokens revoked and local credentials wiped.".into(),
                    created_post: None,
                    created_comment: None,
                },
                "Logged out; tokens revoked and local store wiped.".to_string(),
            ),
            Err(e) => tool_error(e),
        }
    }

    /// Fetch a Reddit feed: the front page, a subreddit, or one of the logged-in
    /// user's multireddits. Returns trimmed posts and an opaque `next_after`
    /// cursor for pagination. Works without login (public content, app-only
    /// access) for the front page and public subreddits.
    #[tool(annotations(title = "Get Reddit feed", read_only_hint = true))]
    async fn reddit_get_feed(
        &self,
        Parameters(p): Parameters<GetFeedParams>,
    ) -> Result<CallToolResult, McpError> {
        let sort = p.sort.clone().unwrap_or(FeedSort::Hot);
        let mut query: Vec<(String, String)> = vec![clamp_limit(p.limit, 100)];
        if let Some(a) = &p.after {
            query.push(("after".into(), a.clone()));
        }
        if sort.uses_time() {
            query.push((
                "t".into(),
                p.time.clone().unwrap_or(TimeRange::Day).value().to_string(),
            ));
        }
        let (path, label) = if let Some(sub) = &p.subreddit {
            (
                format!("/r/{}/{}", clean_sub(sub), sort.path()),
                format!("r/{}", clean_sub(sub)),
            )
        } else if let Some(multi) = &p.multi {
            (
                format!("/user/me/m/{}/{}", clean_user(multi), sort.path()),
                format!("multireddit {}", multi),
            )
        } else {
            (format!("/{}", sort.path()), "the front page".to_string())
        };
        match self.client.get(Base::Oauth, false, &path, &query).await {
            Ok(v) => {
                let feed = models::posts_from_listing(&v);
                let human = format!(
                    "{} posts from {} (sort: {}){}.",
                    feed.posts.len(),
                    label,
                    sort.path(),
                    feed.next_after
                        .as_deref()
                        .map(|a| format!(" next_after={a}"))
                        .unwrap_or_default()
                );
                ok(&feed, human)
            }
            Err(e) => tool_error(e),
        }
    }

    /// Fetch a single post with its comment tree (flattened, with depth).
    /// Works without login for public posts.
    #[tool(annotations(title = "Get Reddit post", read_only_hint = true))]
    async fn reddit_get_post(
        &self,
        Parameters(p): Parameters<GetPostParams>,
    ) -> Result<CallToolResult, McpError> {
        let id36 = p.post_id.trim().trim_start_matches("t3_").to_string();
        if id36.is_empty() || id36.contains('/') {
            return invalid("post_id must be a fullname (t3_...) or bare base36 id");
        }
        let sort = p.comment_sort.clone().unwrap_or(CommentSort::Best);
        let query = vec![
            (
                "limit".into(),
                p.comment_limit.unwrap_or(50).clamp(1, 100).to_string(),
            ),
            ("sort".into(), sort.value().to_string()),
            (
                "depth".into(),
                p.depth.unwrap_or(10).clamp(1, 50).to_string(),
            ),
        ];
        match self
            .client
            .get(Base::Oauth, false, &format!("/comments/{id36}"), &query)
            .await
        {
            Ok(Value::Array(parts)) => {
                let post = parts
                    .first()
                    .and_then(|l| {
                        models::listing(l)
                            .children
                            .iter()
                            .find(|c| models::s(c, "kind") == Some("t3"))
                            .and_then(|c| c.get("data"))
                            .cloned()
                    })
                    .map(|d| models::post_from(&d));
                let mut tree = models::CommentTree::default();
                if let Some(comments_listing) = parts.get(1) {
                    let children = models::listing(comments_listing).children;
                    models::flatten_comments(&children, 0, &mut tree);
                }
                let detail = models::PostDetail {
                    post,
                    comments: tree,
                    comment_sort: Some(sort.value().to_string()),
                };
                let human = format!(
                    "\"{}\" — {} comments fetched ({} more collapsed).",
                    detail
                        .post
                        .as_ref()
                        .map(|x| x.title.text.clone())
                        .unwrap_or_default(),
                    detail.comments.comments.len(),
                    detail.comments.more_children.len()
                );
                ok(&detail, human)
            }
            Ok(_) => tool_error(RedditError::BadShape(
                "expected a two-listing array from /comments".into(),
            )),
            Err(e) => tool_error(e),
        }
    }

    /// Search Reddit (site-wide or within a subreddit) for posts, subreddits,
    /// or users. Supports query operators like author:, selftext:, site:,
    /// nsfw:, self:. Works without login.
    #[tool(annotations(title = "Search Reddit", read_only_hint = true))]
    async fn reddit_search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let (path, mut query) = if let Some(sub) = &p.subreddit {
            (
                format!("/r/{}/search", clean_sub(sub)),
                vec![("restrict_sr".into(), "on".into())],
            )
        } else {
            ("/search".to_string(), vec![])
        };
        query.push(("q".into(), p.query.clone()));
        if let Some(sort) = &p.sort {
            query.push(("sort".into(), sort.clone()));
        }
        if let Some(t) = &p.time {
            query.push(("t".into(), t.value().to_string()));
        }
        if let Some(t) = &p.result_type {
            query.push(("type".into(), t.clone()));
        }
        query.push(clamp_limit(p.limit, 100));
        if let Some(a) = &p.after {
            query.push(("after".into(), a.clone()));
        }
        match self.client.get(Base::Oauth, false, &path, &query).await {
            Ok(v) => {
                let res = models::search_from_listing(&v);
                let human = format!(
                    "Search returned {} posts, {} subreddits, {} users{}.",
                    res.posts.len(),
                    res.subreddits.len(),
                    res.users.len(),
                    res.next_after
                        .as_deref()
                        .map(|a| format!(" (next_after={a})"))
                        .unwrap_or_default()
                );
                ok(&res, human)
            }
            Err(e) => tool_error(e),
        }
    }

    /// Fetch a user's profile: identity (karma, mod status) plus a listing of
    /// their posts and/or comments. The `saved`/`upvoted`/`downvoted`/
    /// `hidden` sections only work for the logged-in user.
    #[tool(annotations(title = "Get Reddit user", read_only_hint = true))]
    async fn reddit_get_user(
        &self,
        Parameters(p): Parameters<GetUserParams>,
    ) -> Result<CallToolResult, McpError> {
        let user = clean_user(&p.username);
        if user.is_empty() {
            return invalid("username is required");
        }
        let section = p.section.clone().unwrap_or(UserSection::Overview);
        let about = match self
            .client
            .get(Base::Oauth, false, &format!("/user/{user}/about"), &[])
            .await
        {
            Ok(v) => Some(models::identity_from(&v)),
            Err(RedditError::Http { status: 404, .. }) => None,
            Err(e) => return tool_error(e),
        };
        let mut detail = models::UserDetail {
            about,
            posts: Vec::new(),
            comments: Vec::new(),
            next_after: None,
        };
        if !matches!(section, UserSection::About) {
            let mut query = vec![clamp_limit(p.limit, 100)];
            if let Some(t) = &p.time {
                query.push(("t".into(), t.value().to_string()));
            }
            if let Some(a) = &p.after {
                query.push(("after".into(), a.clone()));
            }
            let path = format!("/user/{user}/{}", section.path());
            match self.client.get(Base::Oauth, false, &path, &query).await {
                Ok(v) => {
                    let mixed = models::mixed_from_listing(&v);
                    detail.posts = mixed.posts;
                    detail.comments = mixed.comments;
                    detail.next_after = mixed.next_after;
                }
                Err(e) => return tool_error(e),
            }
        }
        let human = format!(
            "u/{user}: {} posts, {} comments{}.",
            detail.posts.len(),
            detail.comments.len(),
            detail
                .next_after
                .as_deref()
                .map(|a| format!(" (next_after={a})"))
                .unwrap_or_default()
        );
        ok(&detail, human)
    }

    /// Fetch subreddit metadata (subscribers, activity, description, your
    /// relationship to it) and optionally its rules. Works without login.
    #[tool(annotations(title = "Get subreddit info", read_only_hint = true))]
    async fn reddit_get_subreddit(
        &self,
        Parameters(p): Parameters<GetSubredditParams>,
    ) -> Result<CallToolResult, McpError> {
        let sub = clean_sub(&p.subreddit);
        if sub.is_empty() {
            return invalid("subreddit is required");
        }
        let about = match self
            .client
            .get(Base::Oauth, false, &format!("/r/{sub}/about"), &[])
            .await
        {
            Ok(v) => models::subreddit_from(v.get("data").unwrap_or(&Value::Null)),
            Err(e) => return tool_error(e),
        };
        let mut rules = Vec::new();
        if p.include_rules.unwrap_or(true)
            && let Ok(v) = self
                .client
                .get(Base::Oauth, false, &format!("/r/{sub}/about/rules"), &[])
                .await
            && let Some(arr) = v.as_array()
        {
            for r in arr {
                let d = r.get("data").unwrap_or(r);
                rules.push(models::SubredditRule {
                    short_name: models::so(d, "short_name").unwrap_or_default(),
                    description: models::truncate(
                        models::s(d, "description").unwrap_or_default(),
                        500,
                    ),
                    violation_reason: models::so(d, "violation_reason"),
                });
            }
        }
        let detail = models::SubredditDetail {
            about: Some(about),
            rules,
        };
        let human = format!(
            "r/{sub}: {} subscribers, {} rules.",
            detail
                .about
                .as_ref()
                .and_then(|a| a.subscribers)
                .map(|x| x.to_string())
                .unwrap_or_else(|| "?".into()),
            detail.rules.len()
        );
        ok(&detail, human)
    }

    /// List inbox messages, comment replies, and mentions (inbox/unread/sent).
    /// Requires login (privatemessages scope).
    #[tool(annotations(title = "Get Reddit inbox", read_only_hint = true))]
    async fn reddit_get_inbox(
        &self,
        Parameters(p): Parameters<GetInboxParams>,
    ) -> Result<CallToolResult, McpError> {
        let box_path = match p.box_.clone().unwrap_or(InboxBox::Inbox) {
            InboxBox::Inbox => "inbox",
            InboxBox::Unread => "unread",
            InboxBox::Sent => "sent",
            InboxBox::Messages => "messages",
        };
        let query = vec![
            (
                "mark".into(),
                if p.mark_read.unwrap_or(false) {
                    "true"
                } else {
                    "false"
                }
                .to_string(),
            ),
            clamp_limit(p.limit, 100),
        ];
        let mut query = query;
        if let Some(a) = &p.after {
            query.push(("after".into(), a.clone()));
        }
        match self
            .client
            .get(Base::Oauth, true, &format!("/message/{box_path}"), &query)
            .await
        {
            Ok(v) => {
                let res = models::messages_from_listing(&v);
                let human = format!(
                    "{} message(s) from {}{}.",
                    res.messages.len(),
                    box_path,
                    res.next_after
                        .as_deref()
                        .map(|a| format!(" (next_after={a})"))
                        .unwrap_or_default()
                );
                ok(&res, human)
            }
            Err(e) => tool_error(e),
        }
    }

    /// List subreddits: your subscriptions/moderated/contributed subs, or the
    /// site-wide popular/new listings (no login needed for the latter).
    #[tool(annotations(title = "List subreddits", read_only_hint = true))]
    async fn reddit_list_subreddits(
        &self,
        Parameters(p): Parameters<ListSubredditsParams>,
    ) -> Result<CallToolResult, McpError> {
        let where_ = p.where_.clone().unwrap_or(SubredditListWhere::Subscriber);
        let mine = matches!(
            where_,
            SubredditListWhere::Subscriber
                | SubredditListWhere::Moderator
                | SubredditListWhere::Contributor
        ) && p.mine.unwrap_or(true);
        let (path, require_user) = match (&where_, mine) {
            (SubredditListWhere::Popular, _) => ("/subreddits/popular".to_string(), false),
            (SubredditListWhere::New, _) => ("/subreddits/new".to_string(), false),
            (w, true) => (
                format!(
                    "/subreddits/mine/{}",
                    match w {
                        SubredditListWhere::Subscriber => "subscriber",
                        SubredditListWhere::Moderator => "moderator",
                        SubredditListWhere::Contributor => "contributor",
                        _ => unreachable!(),
                    }
                ),
                true,
            ),
            (w, false) => (
                format!(
                    "/subreddits/{}",
                    match w {
                        SubredditListWhere::Subscriber => "popular",
                        SubredditListWhere::Moderator => "new",
                        SubredditListWhere::Contributor => "new",
                        _ => unreachable!(),
                    }
                ),
                false,
            ),
        };
        let mut query = vec![clamp_limit(p.limit, 100)];
        if let Some(a) = &p.after {
            query.push(("after".into(), a.clone()));
        }
        match self
            .client
            .get(Base::Oauth, require_user, &path, &query)
            .await
        {
            Ok(v) => {
                let subs: Vec<models::SubredditAbout> = models::listing(&v)
                    .children
                    .iter()
                    .filter(|c| models::s(c, "kind") == Some("t5"))
                    .filter_map(|c| c.get("data"))
                    .map(models::subreddit_from)
                    .collect();
                let res = models::FeedResult {
                    posts: Vec::new(),
                    next_after: models::listing(&v).after,
                };
                let payload = serde_json::json!({
                    "subreddits": subs,
                    "next_after": res.next_after,
                });
                let human = format!("{} subreddits listed.", subs.len());
                ok(&payload, human)
            }
            Err(e) => tool_error(e),
        }
    }

    /// Create a post (text/self or link) in a subreddit. Requires login with
    /// the submit scope. Use reddit_get_subreddit first to check rules and
    /// available link flair.
    #[tool(annotations(title = "Create Reddit post", destructive_hint = true))]
    async fn reddit_create_post(
        &self,
        Parameters(p): Parameters<CreatePostParams>,
    ) -> Result<CallToolResult, McpError> {
        let sub = clean_sub(&p.subreddit);
        if sub.is_empty() {
            return invalid("subreddit is required");
        }
        if p.title.trim().is_empty() || p.title.chars().count() > 300 {
            return invalid("title must be 1–300 characters");
        }
        let (kind, text, url) = match &p.kind {
            PostKind::Text => {
                let text = p.text.clone().unwrap_or_default();
                ("self", Some(text), None::<String>)
            }
            PostKind::Link => {
                let Some(url) = p.url.clone().filter(|u| u.starts_with("http")) else {
                    return invalid("link posts require a url starting with http(s)://");
                };
                ("link", None, Some(url))
            }
        };
        let mut form = vec![
            ("sr".into(), sub.clone()),
            ("kind".into(), kind.to_string()),
            ("title".into(), p.title.clone()),
            (
                "resubmit".into(),
                if p.resubmit.unwrap_or(false) {
                    "true"
                } else {
                    "false"
                }
                .into(),
            ),
            (
                "sendreplies".into(),
                if p.sendreplies.unwrap_or(true) {
                    "true"
                } else {
                    "false"
                }
                .into(),
            ),
        ];
        if let Some(t) = text {
            form.push(("text".into(), t));
        }
        if let Some(u) = url {
            form.push(("url".into(), u));
        }
        if let Some(f) = &p.flair_id {
            form.push(("flair_id".into(), f.clone()));
        }
        match self.client.post("/api/submit", form).await {
            Ok(v) => {
                let created = first_thing(&v).map(|d| models::post_from(&d));
                let detail = created
                    .as_ref()
                    .map(|x| x.permalink.clone().unwrap_or_else(|| x.id.clone()))
                    .unwrap_or_else(|| format!("r/{sub}"));
                let res = models::ActionResult {
                    ok: true,
                    detail: format!("Posted to r/{sub}: {detail}"),
                    created_post: created,
                    created_comment: None,
                };
                ok(&res, res.detail.clone())
            }
            Err(e) => tool_error(e),
        }
    }

    /// Comment on a post (parent t3_...) or reply to a comment (parent
    /// t1_...). Body is Markdown, max 10,000 chars. Requires login.
    #[tool(annotations(title = "Create Reddit comment", destructive_hint = true))]
    async fn reddit_create_comment(
        &self,
        Parameters(p): Parameters<CreateCommentParams>,
    ) -> Result<CallToolResult, McpError> {
        if !p.parent_id.starts_with("t1_") && !p.parent_id.starts_with("t3_") {
            return invalid("parent_id must be a fullname: t3_... (post) or t1_... (comment)");
        }
        if p.text.trim().is_empty() || p.text.chars().count() > 10_000 {
            return invalid("text must be 1–10,000 characters");
        }
        match self
            .client
            .post(
                "/api/comment",
                vec![
                    ("thing_id".into(), p.parent_id.clone()),
                    ("text".into(), p.text.clone()),
                ],
            )
            .await
        {
            Ok(v) => {
                let created = first_thing(&v).map(|d| models::comment_from(&d, 0));
                let res = models::ActionResult {
                    ok: true,
                    detail: format!(
                        "Comment posted: {}",
                        created.as_ref().map(|c| c.id.clone()).unwrap_or_default()
                    ),
                    created_post: None,
                    created_comment: created,
                };
                ok(&res, res.detail.clone())
            }
            Err(e) => tool_error(e),
        }
    }

    /// Edit or delete your own comment (t1_...) or self post (t3_...).
    /// Edits require the new Markdown text; Reddit throttles edits and deletes
    /// of young content. Requires login (edit scope).
    #[tool(annotations(title = "Edit or delete content", destructive_hint = true))]
    async fn reddit_edit_or_delete(
        &self,
        Parameters(p): Parameters<EditOrDeleteParams>,
    ) -> Result<CallToolResult, McpError> {
        if !p.thing_id.starts_with("t1_") && !p.thing_id.starts_with("t3_") {
            return invalid("thing_id must be t1_... or t3_...");
        }
        match p.action {
            EditOrDeleteAction::Edit => {
                let Some(text) = p.text.filter(|t| !t.trim().is_empty()) else {
                    return invalid("edit requires non-empty text");
                };
                match self
                    .client
                    .post(
                        "/api/editusertext",
                        vec![
                            ("thing_id".into(), p.thing_id.clone()),
                            ("text".into(), text),
                        ],
                    )
                    .await
                {
                    Ok(_) => ok(
                        &models::ActionResult {
                            ok: true,
                            detail: format!("Edited {}.", p.thing_id),
                            created_post: None,
                            created_comment: None,
                        },
                        format!("Edited {}.", p.thing_id),
                    ),
                    Err(e) => tool_error(e),
                }
            }
            EditOrDeleteAction::Delete => {
                match self
                    .client
                    .post("/api/del", vec![("id".into(), p.thing_id.clone())])
                    .await
                {
                    Ok(_) => ok(
                        &models::ActionResult {
                            ok: true,
                            detail: format!("Deleted {}.", p.thing_id),
                            created_post: None,
                            created_comment: None,
                        },
                        format!("Deleted {}.", p.thing_id),
                    ),
                    Err(e) => tool_error(e),
                }
            }
        }
    }

    /// Interact with an existing post or comment: vote (up/down/clear), save,
    /// unsave, hide, unhide, or report to moderators/admins. Requires login.
    #[tool(annotations(title = "Vote/save/hide/report item", destructive_hint = true))]
    async fn reddit_item_action(
        &self,
        Parameters(p): Parameters<ItemActionParams>,
    ) -> Result<CallToolResult, McpError> {
        if !p.thing_id.starts_with("t1_") && !p.thing_id.starts_with("t3_") {
            return invalid("thing_id must be t1_... or t3_...");
        }
        let (path, form): (&str, Vec<(String, String)>) = match &p.action {
            ItemAction::Vote => {
                let Some(dir) = &p.direction else {
                    return invalid("vote requires a direction");
                };
                (
                    "/api/vote",
                    vec![
                        ("id".into(), p.thing_id.clone()),
                        ("dir".into(), dir.value().to_string()),
                    ],
                )
            }
            ItemAction::Save => {
                let mut form = vec![("id".into(), p.thing_id.clone())];
                if let Some(c) = &p.category {
                    form.push(("category".into(), c.clone()));
                }
                ("/api/save", form)
            }
            ItemAction::Unsave => ("/api/unsave", vec![("id".into(), p.thing_id.clone())]),
            ItemAction::Hide => ("/api/hide", vec![("id".into(), p.thing_id.clone())]),
            ItemAction::Unhide => ("/api/unhide", vec![("id".into(), p.thing_id.clone())]),
            ItemAction::Report => {
                if p.reason.as_deref().map(str::is_empty).unwrap_or(true)
                    && p.site_reason.as_deref().map(str::is_empty).unwrap_or(true)
                {
                    return invalid("report requires a reason or site_reason (e.g. Spam)");
                }
                let mut form = vec![("thing_id".into(), p.thing_id.clone())];
                if let Some(r) = &p.reason {
                    form.push(("custom_text".into(), r.chars().take(100).collect()));
                    form.push(("reason".into(), r.chars().take(100).collect()));
                }
                if let Some(sr) = &p.site_reason {
                    form.push(("site_reason".into(), sr.clone()));
                }
                ("/api/report", form)
            }
        };
        match self.client.post(path, form).await {
            Ok(_) => ok(
                &models::ActionResult {
                    ok: true,
                    detail: format!("{:?} applied to {}.", p.action, p.thing_id),
                    created_post: None,
                    created_comment: None,
                },
                format!("{:?} applied to {}.", p.action, p.thing_id),
            ),
            Err(e) => tool_error(e),
        }
    }

    /// Set flair: link flair on a post (t3_), or user flair in a subreddit
    /// (defaults to the logged-in user; moderators can set others'). Call
    /// with no template id or text to list the available flair options first.
    #[tool(annotations(title = "Set Reddit flair", destructive_hint = true))]
    async fn reddit_set_flair(
        &self,
        Parameters(p): Parameters<SetFlairParams>,
    ) -> Result<CallToolResult, McpError> {
        let sub = clean_sub(&p.subreddit);
        if sub.is_empty() {
            return invalid("subreddit is required");
        }
        if p.flair_template_id.is_none() && p.text.is_none() {
            // List available flair options instead.
            let path = if p.post_id.is_some() {
                format!("/r/{sub}/api/link_flair_v2")
            } else {
                format!("/r/{sub}/api/user_flair_v2")
            };
            return match self
                .client
                .get(Base::Oauth, true, &path, &[("limit".into(), "100".into())])
                .await
            {
                Ok(v) => {
                    let options: Vec<FlairOption> = models::listing(&v)
                        .children
                        .iter()
                        .filter_map(|c| c.get("data"))
                        .map(|d| FlairOption {
                            id: models::so(d, "id"),
                            text: models::so(d, "text"),
                            editable: models::b(d, "editable"),
                        })
                        .collect();
                    let human = format!(
                        "{} {} flair options for r/{sub}; call again with flair_template_id.",
                        options.len(),
                        if p.post_id.is_some() { "link" } else { "user" }
                    );
                    let payload = serde_json::json!({ "flair_options": options });
                    ok(&payload, human)
                }
                Err(e) => tool_error(e),
            };
        }
        let mut form: Vec<(String, String)> = Vec::new();
        if let Some(id) = &p.flair_template_id {
            form.push(("flair_template_id".into(), id.clone()));
        }
        if let Some(t) = &p.text {
            form.push(("text".into(), t.clone()));
        }
        if let Some(post) = &p.post_id {
            if !post.starts_with("t3_") {
                return invalid("post_id must be a t3_ fullname");
            }
            form.push(("link".into(), post.clone()));
        } else {
            let name = match &p.username {
                Some(u) => clean_user(u),
                None => match self.client.current_username().await {
                    Some(u) => u,
                    None => return tool_error(RedditError::NotLoggedIn),
                },
            };
            form.push(("name".into(), name));
        }
        match self
            .client
            .post(&format!("/r/{sub}/api/selectflair"), form)
            .await
        {
            Ok(_) => ok(
                &models::ActionResult {
                    ok: true,
                    detail: format!("Flair set in r/{sub}."),
                    created_post: None,
                    created_comment: None,
                },
                format!("Flair set in r/{sub}."),
            ),
            Err(e) => tool_error(e),
        }
    }

    /// Subscribe or unsubscribe one or more subreddits. Requires login
    /// (subscribe scope).
    #[tool(annotations(title = "Subscribe to subreddits", idempotent_hint = true))]
    async fn reddit_subscribe(
        &self,
        Parameters(p): Parameters<SubscribeParams>,
    ) -> Result<CallToolResult, McpError> {
        let subs: Vec<String> = p
            .subreddits
            .iter()
            .map(|s| clean_sub(s))
            .filter(|s| !s.is_empty())
            .collect();
        if subs.is_empty() {
            return invalid("at least one subreddit is required");
        }
        let action = match p.action {
            SubscribeAction::Subscribe => "sub",
            SubscribeAction::Unsubscribe => "unsub",
        };
        let form = vec![
            ("action".into(), action.to_string()),
            ("sr_name".into(), subs.join(",")),
        ];
        match self.client.post("/api/subscribe", form).await {
            Ok(_) => ok(
                &models::ActionResult {
                    ok: true,
                    detail: format!("{action} {}", subs.join(", ")),
                    created_post: None,
                    created_comment: None,
                },
                format!("{action} {}", subs.join(", ")),
            ),
            Err(e) => tool_error(e),
        }
    }

    /// Send a private message to a Reddit user (or, with from_subreddit, as a
    /// subreddit you moderate). Requires login (privatemessages scope).
    #[tool(annotations(title = "Send private message", destructive_hint = true))]
    async fn reddit_send_message(
        &self,
        Parameters(p): Parameters<SendMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        let to = clean_user(&p.to);
        if to.is_empty() || p.subject.trim().is_empty() || p.body.trim().is_empty() {
            return invalid("to, subject, and body are all required");
        }
        if p.subject.chars().count() > 100 {
            return invalid("subject must be at most 100 characters");
        }
        let mut form = vec![
            ("to".into(), to.clone()),
            (
                "subject".into(),
                p.subject.chars().take(100).collect::<String>(),
            ),
            ("text".into(), p.body.clone()),
        ];
        if let Some(from) = &p.from_subreddit {
            form.push(("from_sr".into(), clean_sub(from)));
        }
        match self.client.post("/api/compose", form).await {
            Ok(_) => ok(
                &models::ActionResult {
                    ok: true,
                    detail: format!("Private message sent to u/{to}."),
                    created_post: None,
                    created_comment: None,
                },
                format!("Private message sent to u/{to}."),
            ),
            Err(e) => tool_error(e),
        }
    }

    /// Mark private messages / comment replies as read, either by fullname
    /// list or the entire inbox. Requires login.
    #[tool(annotations(title = "Mark messages read", idempotent_hint = true))]
    async fn reddit_mark_read(
        &self,
        Parameters(p): Parameters<MarkReadParams>,
    ) -> Result<CallToolResult, McpError> {
        if p.all.unwrap_or(false) {
            return match self.client.post("/api/read_all_messages", vec![]).await {
                Ok(_) => ok(
                    &models::ActionResult {
                        ok: true,
                        detail: "Inbox marked read.".into(),
                        created_post: None,
                        created_comment: None,
                    },
                    "Inbox marked read.".to_string(),
                ),
                Err(e) => tool_error(e),
            };
        }
        let Some(ids) = p.message_ids.filter(|v| !v.is_empty()) else {
            return invalid("provide message_ids or all=true");
        };
        match self
            .client
            .post("/api/read_message", vec![("id".into(), ids.join(","))])
            .await
        {
            Ok(_) => ok(
                &models::ActionResult {
                    ok: true,
                    detail: format!("Marked {} message(s) read.", ids.len()),
                    created_post: None,
                    created_comment: None,
                },
                format!("Marked {} message(s) read.", ids.len()),
            ),
            Err(e) => tool_error(e),
        }
    }

    /// List a moderation queue for a subreddit you moderate: modqueue, user
    /// reports, spam, unmoderated, or recently edited items. Requires the
    /// modposts scope and moderator status.
    #[tool(annotations(title = "List mod queue", read_only_hint = true))]
    async fn reddit_mod_queue(
        &self,
        Parameters(p): Parameters<ModQueueParams>,
    ) -> Result<CallToolResult, McpError> {
        let sub = clean_sub(&p.subreddit);
        if sub.is_empty() {
            return invalid("subreddit is required");
        }
        let location = p.location.clone().unwrap_or(ModQueueLocation::Modqueue);
        let mut query = vec![clamp_limit(p.limit, 100)];
        if let Some(only) = &p.only {
            if only != "links" && only != "comments" {
                return invalid("only must be \"links\" or \"comments\"");
            }
            query.push(("only".into(), only.clone()));
        }
        if let Some(a) = &p.after {
            query.push(("after".into(), a.clone()));
        }
        let path = format!("/r/{sub}/about/{}", location.path());
        match self.client.get(Base::Oauth, true, &path, &query).await {
            Ok(v) => {
                let res = models::mixed_from_listing(&v);
                let human = format!(
                    "r/{sub} {} queue: {} posts, {} comments{}.",
                    location.path(),
                    res.posts.len(),
                    res.comments.len(),
                    res.next_after
                        .as_deref()
                        .map(|a| format!(" (next_after={a})"))
                        .unwrap_or_default()
                );
                ok(&res, human)
            }
            Err(e) => tool_error(e),
        }
    }

    /// Fetch the moderator action log for a subreddit you moderate
    /// (modlog scope), optionally filtered by action type or moderator.
    #[tool(annotations(title = "Get mod log", read_only_hint = true))]
    async fn reddit_mod_log(
        &self,
        Parameters(p): Parameters<ModLogParams>,
    ) -> Result<CallToolResult, McpError> {
        let sub = clean_sub(&p.subreddit);
        if sub.is_empty() {
            return invalid("subreddit is required");
        }
        let mut query = vec![clamp_limit(p.limit, 500)];
        if let Some(t) = &p.action_type {
            query.push(("type".into(), t.clone()));
        }
        if let Some(m) = &p.mod_ {
            query.push(("mod".into(), clean_user(m)));
        }
        if let Some(a) = &p.after {
            query.push(("after".into(), a.clone()));
        }
        let path = format!("/r/{sub}/about/log");
        match self.client.get(Base::Oauth, true, &path, &query).await {
            Ok(v) => {
                let entries = models::modlog_from_listing(&v);
                let after = models::listing(&v).after;
                let human = format!(
                    "{} mod log entries for r/{sub}{}.",
                    entries.len(),
                    after
                        .as_deref()
                        .map(|a| format!(" (next_after={a})"))
                        .unwrap_or_default()
                );
                let payload = serde_json::json!({ "entries": entries, "next_after": after });
                ok(&payload, human)
            }
            Err(e) => tool_error(e),
        }
    }

    /// Perform a moderation action on a post or comment: remove, remove as
    /// spam, approve, distinguish/undistinguish (optionally sticky), sticky/
    /// unsticky, lock/unlock, ignore/unignore reports, mark/unmark NSFW or
    /// spoiler, or set the suggested comment sort. Requires modposts scope
    /// and moderator status.
    #[tool(annotations(title = "Perform mod action", destructive_hint = true))]
    async fn reddit_mod_action(
        &self,
        Parameters(p): Parameters<ModActionParams>,
    ) -> Result<CallToolResult, McpError> {
        if !p.thing_id.starts_with("t1_") && !p.thing_id.starts_with("t3_") {
            return invalid("thing_id must be t1_... or t3_...");
        }
        let form: Vec<(String, String)> = match &p.action {
            ModActionKind::Remove => vec![
                ("id".into(), p.thing_id.clone()),
                ("spam".into(), "false".into()),
            ],
            ModActionKind::Spam => vec![
                ("id".into(), p.thing_id.clone()),
                ("spam".into(), "true".into()),
            ],
            ModActionKind::Approve => vec![("id".into(), p.thing_id.clone())],
            ModActionKind::Distinguish | ModActionKind::Undistinguish => {
                let how = match &p.action {
                    ModActionKind::Undistinguish => "no".to_string(),
                    _ => match &p.distinguish_how {
                        Some(DistinguishHow::Yes) | None => "yes".to_string(),
                        Some(DistinguishHow::Admin) => "admin".to_string(),
                        Some(DistinguishHow::Special) => "special".to_string(),
                    },
                };
                let mut form = vec![("id".into(), p.thing_id.clone()), ("how".into(), how)];
                if p.sticky.unwrap_or(false) {
                    form.push(("sticky".into(), "true".into()));
                }
                form
            }
            ModActionKind::Sticky => vec![
                ("id".into(), p.thing_id.clone()),
                ("state".into(), "true".into()),
            ],
            ModActionKind::Unsticky => vec![
                ("id".into(), p.thing_id.clone()),
                ("state".into(), "false".into()),
            ],
            ModActionKind::Lock => vec![("id".into(), p.thing_id.clone())],
            ModActionKind::Unlock => vec![("id".into(), p.thing_id.clone())],
            ModActionKind::IgnoreReports | ModActionKind::UnignoreReports => {
                vec![("id".into(), p.thing_id.clone())]
            }
            ModActionKind::MarkNsfw
            | ModActionKind::UnmarkNsfw
            | ModActionKind::MarkSpoiler
            | ModActionKind::UnmarkSpoiler => vec![("id".into(), p.thing_id.clone())],
            ModActionKind::SuggestedSort => {
                let Some(sort) = &p.suggested_sort else {
                    return invalid("suggested_sort requires the sort value");
                };
                vec![
                    ("id".into(), p.thing_id.clone()),
                    ("sort".into(), sort.value().to_string()),
                ]
            }
        };
        let path = match &p.action {
            ModActionKind::Remove | ModActionKind::Spam => "/api/remove",
            ModActionKind::Approve => "/api/approve",
            ModActionKind::Distinguish | ModActionKind::Undistinguish => "/api/distinguish",
            ModActionKind::Sticky | ModActionKind::Unsticky => "/api/set_subreddit_sticky",
            ModActionKind::Lock => "/api/lock",
            ModActionKind::Unlock => "/api/unlock",
            ModActionKind::IgnoreReports => "/api/ignore_reports",
            ModActionKind::UnignoreReports => "/api/unignore_reports",
            ModActionKind::MarkNsfw => "/api/marknsfw",
            ModActionKind::UnmarkNsfw => "/api/unmarknsfw",
            ModActionKind::MarkSpoiler => "/api/spoiler",
            ModActionKind::UnmarkSpoiler => "/api/unspoiler",
            ModActionKind::SuggestedSort => "/api/suggested_sort",
        };
        match self.client.post(path, form).await {
            Ok(_) => ok(
                &models::ActionResult {
                    ok: true,
                    detail: format!("{:?} applied to {}.", p.action, p.thing_id),
                    created_post: None,
                    created_comment: None,
                },
                format!("{:?} applied to {}.", p.action, p.thing_id),
            ),
            Err(e) => tool_error(e),
        }
    }

    /// Add or remove a subreddit role for a user: ban, mute, moderator
    /// (adding sends an invite the user must accept), moderator-invite
    /// removal, or contributor. Bans accept duration/reason/note/message;
    /// mod invites accept a permissions string like `+all` or `+posts,+mail`.
    /// Requires the appropriate mod scopes and moderator status.
    #[tool(annotations(title = "Manage subreddit user role", destructive_hint = true))]
    async fn reddit_mod_user(
        &self,
        Parameters(p): Parameters<ModUserParams>,
    ) -> Result<CallToolResult, McpError> {
        let sub = clean_sub(&p.subreddit);
        let user = clean_user(&p.username);
        if sub.is_empty() || user.is_empty() {
            return invalid("subreddit and username are required");
        }
        let friend_type = match (&p.role, &p.action) {
            (ModRole::Banned, AddRemove::Add) => "banned",
            (ModRole::Banned, AddRemove::Remove) => "banned",
            (ModRole::Muted, AddRemove::Add) => "muted",
            (ModRole::Muted, AddRemove::Remove) => "muted",
            (ModRole::Moderator, AddRemove::Add) => "moderator_invite",
            (ModRole::Moderator, AddRemove::Remove) => "moderator",
            (ModRole::ModeratorInvite, AddRemove::Add) => "moderator_invite",
            (ModRole::ModeratorInvite, AddRemove::Remove) => "moderator_invite",
            (ModRole::Contributor, AddRemove::Add) => "contributor",
            (ModRole::Contributor, AddRemove::Remove) => "contributor",
        };
        let (path, mut form) = match p.action {
            AddRemove::Add => (
                format!("/r/{sub}/api/friend"),
                vec![
                    ("name".into(), user.clone()),
                    ("type".into(), friend_type.to_string()),
                ],
            ),
            AddRemove::Remove => (
                format!("/r/{sub}/api/unfriend"),
                vec![
                    ("name".into(), user.clone()),
                    ("type".into(), friend_type.to_string()),
                ],
            ),
        };
        if let Some(reason) = &p.reason {
            form.push(("ban_reason".into(), reason.chars().take(100).collect()));
        }
        if let Some(note) = &p.note {
            form.push(("note".into(), note.chars().take(300).collect()));
        }
        if let Some(msg) = &p.message_to_user {
            form.push(("ban_message".into(), msg.chars().take(1000).collect()));
        }
        if let Some(d) = p.duration_days {
            form.push(("ban_duration".into(), d.min(999).to_string()));
        }
        if let Some(perms) = &p.permissions {
            form.push(("permissions".into(), perms.clone()));
        }
        let verb = match p.action {
            AddRemove::Add => "added",
            AddRemove::Remove => "removed",
        };
        match self.client.post(&path, form).await {
            Ok(_) => ok(
                &models::ActionResult {
                    ok: true,
                    detail: format!("{friend_type} {verb} for u/{user} in r/{sub}."),
                    created_post: None,
                    created_comment: None,
                },
                format!("{friend_type} {verb} for u/{user} in r/{sub}."),
            ),
            Err(e) => tool_error(e),
        }
    }

    /// List users holding a role in a subreddit you moderate: banned, muted,
    /// moderators, or contributors (modcontributors scope).
    #[tool(annotations(title = "List subreddit users by role", read_only_hint = true))]
    async fn reddit_mod_users(
        &self,
        Parameters(p): Parameters<ModUsersParams>,
    ) -> Result<CallToolResult, McpError> {
        let sub = clean_sub(&p.subreddit);
        if sub.is_empty() {
            return invalid("subreddit is required");
        }
        let mut query = vec![clamp_limit(p.limit, 100)];
        if let Some(u) = &p.username_filter {
            query.push(("user".into(), clean_user(u)));
        }
        if let Some(a) = &p.after {
            query.push(("after".into(), a.clone()));
        }
        let path = format!("/r/{sub}/about/{}", p.role.path());
        match self.client.get(Base::Oauth, true, &path, &query).await {
            Ok(v) => {
                let users = models::modusers_from_listing(&v);
                let after = models::listing(&v).after;
                let human = format!(
                    "{} {} in r/{sub}{}.",
                    users.len(),
                    p.role.path(),
                    after
                        .as_deref()
                        .map(|a| format!(" (next_after={a})"))
                        .unwrap_or_default()
                );
                let payload = serde_json::json!({ "users": users, "next_after": after });
                ok(&payload, human)
            }
            Err(e) => tool_error(e),
        }
    }

    /// New modmail: list conversations (filter by state/sort/entities), get a
    /// conversation with its messages, or reply (optionally on the internal
    /// mod-only thread or with hidden mod identity). Requires the modmail
    /// scope and moderator status.
    #[tool(annotations(title = "Reddit modmail"))]
    async fn reddit_modmail(
        &self,
        Parameters(p): Parameters<ModmailParams>,
    ) -> Result<CallToolResult, McpError> {
        match p.action {
            ModmailAction::List => {
                let mut query = vec![
                    (
                        "state".into(),
                        p.state
                            .clone()
                            .unwrap_or(ModmailState::All)
                            .value()
                            .to_string(),
                    ),
                    (
                        "sort".into(),
                        p.sort
                            .clone()
                            .unwrap_or(ModmailSort::Recent)
                            .value()
                            .to_string(),
                    ),
                    clamp_limit(p.limit, 100),
                ];
                if let Some(entity) = &p.entity {
                    query.push(("entity".into(), entity.join(",")));
                }
                match self
                    .client
                    .get(Base::Modmail, true, "/api/mod/conversations", &query)
                    .await
                {
                    Ok(v) => {
                        let mut convs: Vec<models::ModmailConversation> = v
                            .get("conversations")
                            .and_then(|c| c.as_object())
                            .map(|map| {
                                map.iter()
                                    .map(|(id, c)| models::modmail_conversation_from(id, c))
                                    .collect()
                            })
                            .unwrap_or_default();
                        convs.sort_by(|a, b| {
                            b.last_updated_utc
                                .partial_cmp(&a.last_updated_utc)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let human = format!("{} modmail conversations.", convs.len());
                        let payload = serde_json::json!({ "conversations": convs });
                        ok(&payload, human)
                    }
                    Err(e) => tool_error(e),
                }
            }
            ModmailAction::Get => {
                let Some(id) = p.conversation_id.clone().filter(|x| !x.trim().is_empty()) else {
                    return invalid("conversation_id is required");
                };
                let query = vec![(
                    "markRead".into(),
                    if p.mark_read.unwrap_or(true) {
                        "true"
                    } else {
                        "false"
                    }
                    .into(),
                )];
                match self
                    .client
                    .get(
                        Base::Modmail,
                        true,
                        &format!("/api/mod/conversations/{id}"),
                        &query,
                    )
                    .await
                {
                    Ok(v) => {
                        let thread = models::modmail_thread_from(&v);
                        let human =
                            format!("Conversation {id}: {} messages.", thread.messages.len());
                        ok(&thread, human)
                    }
                    Err(e) => tool_error(e),
                }
            }
            ModmailAction::Reply => {
                let Some(id) = p.conversation_id.clone().filter(|x| !x.trim().is_empty()) else {
                    return invalid("conversation_id is required");
                };
                let Some(body) = p.body.clone().filter(|x| !x.trim().is_empty()) else {
                    return invalid("reply requires a body");
                };
                let form = vec![
                    ("body".into(), body),
                    (
                        "isInternal".into(),
                        if p.is_internal.unwrap_or(false) {
                            "true"
                        } else {
                            "false"
                        }
                        .into(),
                    ),
                    (
                        "isAuthorHidden".into(),
                        if p.hide_mod_identity.unwrap_or(false) {
                            "true"
                        } else {
                            "false"
                        }
                        .into(),
                    ),
                ];
                match self
                    .client
                    .request_modmail_post(&format!("/api/mod/conversations/{id}"), form)
                    .await
                {
                    Ok(v) => {
                        let thread = models::modmail_thread_from(&v);
                        let res = models::ActionResult {
                            ok: true,
                            detail: format!("Replied to modmail conversation {id}."),
                            created_post: None,
                            created_comment: None,
                        };
                        let payload = serde_json::json!({ "result": res, "thread": thread });
                        ok(&payload, res.detail.clone())
                    }
                    Err(e) => tool_error(e),
                }
            }
        }
    }
}

#[tool_handler(router = self.tool_router.clone())]
impl ServerHandler for RedditServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "Reddit control over OAuth with the user's own client ID \
                 (installed-app authorization code flow). \
                 First use: call reddit_auth_status — if not logged in, call \
                 reddit_login_start, have the user open the URL and authorize, then pass the \
                 copied code (or full redirect URL) to reddit_login_complete. \
                 Read tools work without login on public content; everything else needs the \
                 login. Listings paginate with the opaque next_after cursor."
                    .to_string(),
            )
    }
}
