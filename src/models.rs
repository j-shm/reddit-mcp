//! Trimmed, agent-friendly models parsed out of Reddit's raw JSON responses.
//!
//! Reddit responses are large and noisy; everything returned to the model goes
//! through these shapes so output stays small: bodies are truncated, unused
//! fields dropped, and listings carry an opaque `next_after` cursor.

use serde::Serialize;
use serde_json::Value;

/// Longest any single text field is allowed to grow before truncation.
pub const MAX_TEXT: usize = 2000;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TruncatedText {
    pub text: String,
    pub truncated: bool,
}

pub fn truncate(s: &str, max: usize) -> TruncatedText {
    if s.chars().count() <= max {
        TruncatedText {
            text: s.to_string(),
            truncated: false,
        }
    } else {
        TruncatedText {
            text: format!("{}… [truncated]", s.chars().take(max).collect::<String>()),
            truncated: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Value accessors (tolerant of Reddit's loose typing)
// ---------------------------------------------------------------------------

pub fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

pub fn so(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(x)) if !x.is_empty() => Some(x.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

pub fn n(v: &Value, k: &str) -> Option<i64> {
    v.get(k)
        .and_then(|x| x.as_i64())
        .or_else(|| v.get(k).and_then(|x| x.as_f64()).map(|f| f as i64))
}

pub fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}

pub fn b(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(|x| x.as_bool())
}

pub fn absolutize(permalink: Option<String>) -> Option<String> {
    permalink.map(|p| {
        if p.starts_with("http") {
            p
        } else {
            format!("https://reddit.com{p}")
        }
    })
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Identity {
    pub name: String,
    pub id: Option<String>,
    pub link_karma: Option<i64>,
    pub comment_karma: Option<i64>,
    pub created_utc: Option<f64>,
    pub is_mod: Option<bool>,
    pub is_gold: Option<bool>,
    pub has_verified_email: Option<bool>,
    pub inbox_count: Option<i64>,
    pub icon_img: Option<String>,
}

pub fn identity_from(v: &Value) -> Identity {
    Identity {
        name: so(v, "name").unwrap_or_default(),
        id: so(v, "id"),
        link_karma: n(v, "link_karma"),
        comment_karma: n(v, "comment_karma"),
        created_utc: f(v, "created_utc"),
        is_mod: b(v, "is_mod"),
        is_gold: b(v, "is_gold"),
        has_verified_email: b(v, "has_verified_email"),
        inbox_count: n(v, "inbox_count"),
        icon_img: so(v, "icon_img"),
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PostSummary {
    /// Fullname (t3_...)
    pub id: String,
    pub subreddit: Option<String>,
    pub author: Option<String>,
    pub title: TruncatedText,
    pub permalink: Option<String>,
    pub url: Option<String>,
    pub is_self: Option<bool>,
    pub selftext: TruncatedText,
    pub score: Option<i64>,
    pub upvote_ratio: Option<f64>,
    pub num_comments: Option<i64>,
    pub created_utc: Option<f64>,
    pub over_18: Option<bool>,
    pub spoiler: Option<bool>,
    pub locked: Option<bool>,
    pub archived: Option<bool>,
    pub stickied: Option<bool>,
    pub edited: Option<bool>,
    pub distinguished: Option<String>,
    pub link_flair_text: Option<String>,
    pub saved: Option<bool>,
    pub hidden: Option<bool>,
    pub likes: Option<i64>,
    /// Report reasons when viewed from a moderation listing.
    pub report_reasons: Vec<String>,
}

pub fn post_from(d: &Value) -> PostSummary {
    let mut report_reasons = Vec::new();
    for r in d
        .get("user_reports")
        .and_then(|x| x.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
    {
        if let Some(reason) = r
            .as_array()
            .and_then(|a| a.first())
            .and_then(|x| x.as_str())
            && !reason.is_empty()
        {
            report_reasons.push(format!("user: {reason}"));
        }
    }
    for r in d
        .get("mod_reports")
        .and_then(|x| x.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
    {
        if let Some(reason) = r
            .as_array()
            .and_then(|a| a.first())
            .and_then(|x| x.as_str())
            && !reason.is_empty()
        {
            report_reasons.push(format!("mod: {reason}"));
        }
    }
    PostSummary {
        id: so(d, "name").unwrap_or_default(),
        subreddit: so(d, "subreddit"),
        author: so(d, "author"),
        title: truncate(s(d, "title").unwrap_or_default(), 300),
        permalink: absolutize(so(d, "permalink")),
        url: so(d, "url"),
        is_self: b(d, "is_self"),
        selftext: truncate(s(d, "selftext").unwrap_or_default(), MAX_TEXT),
        score: n(d, "score"),
        upvote_ratio: f(d, "upvote_ratio"),
        num_comments: n(d, "num_comments"),
        created_utc: f(d, "created_utc"),
        over_18: b(d, "over_18"),
        spoiler: b(d, "spoiler"),
        locked: b(d, "locked"),
        archived: b(d, "archived"),
        stickied: b(d, "stickied"),
        edited: Some(
            d.get("edited")
                .map(|e| !e.is_boolean() || *e == Value::Bool(true))
                .unwrap_or(false),
        ),
        distinguished: so(d, "distinguished"),
        link_flair_text: so(d, "link_flair_text"),
        saved: b(d, "saved"),
        hidden: b(d, "hidden"),
        likes: d.get("likes").and_then(|x| {
            if x.is_null() {
                None
            } else {
                x.as_i64().or(if x.as_bool().unwrap_or(false) {
                    Some(1)
                } else {
                    Some(-1)
                })
            }
        }),
        report_reasons,
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CommentSummary {
    /// Fullname (t1_...)
    pub id: String,
    pub link_id: Option<String>,
    pub parent_id: Option<String>,
    pub author: Option<String>,
    pub body: TruncatedText,
    pub score: Option<i64>,
    pub created_utc: Option<f64>,
    pub edited: Option<bool>,
    pub is_submitter: Option<bool>,
    pub distinguished: Option<String>,
    pub stickied: Option<bool>,
    pub locked: Option<bool>,
    /// Nesting depth; 0 is a top-level comment.
    pub depth: u32,
    pub permalink: Option<String>,
    pub link_title: Option<String>,
    pub link_permalink: Option<String>,
    pub likes: Option<bool>,
    pub report_reasons: Vec<String>,
    /// Number of collapsed ("load more") descendants that were not fetched.
    pub more_children_count: u64,
}

pub fn comment_from(d: &Value, depth: u32) -> CommentSummary {
    let mut report_reasons = Vec::new();
    if let Some(arr) = d.get("user_reports").and_then(|x| x.as_array()) {
        for r in arr {
            if let Some(reason) = r
                .as_array()
                .and_then(|a| a.first())
                .and_then(|x| x.as_str())
                && !reason.is_empty()
            {
                report_reasons.push(format!("user: {reason}"));
            }
        }
    }
    CommentSummary {
        id: so(d, "name").unwrap_or_default(),
        link_id: so(d, "link_id"),
        parent_id: so(d, "parent_id"),
        author: so(d, "author"),
        body: truncate(s(d, "body").unwrap_or("[deleted]"), MAX_TEXT),
        score: n(d, "score"),
        created_utc: f(d, "created_utc"),
        edited: Some(
            d.get("edited")
                .map(|e| !e.is_boolean() || *e == Value::Bool(true))
                .unwrap_or(false),
        ),
        is_submitter: b(d, "is_submitter"),
        distinguished: so(d, "distinguished"),
        stickied: b(d, "stickied"),
        locked: b(d, "locked"),
        depth,
        permalink: absolutize(so(d, "permalink")),
        link_title: so(d, "link_title"),
        link_permalink: absolutize(so(d, "link_permalink")),
        likes: b(d, "likes"),
        report_reasons,
        more_children_count: 0,
    }
}

/// Flattened comment tree with depth information.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct CommentTree {
    pub comments: Vec<CommentSummary>,
    /// Fullnames of "load more" placeholders encountered.
    pub more_children: Vec<String>,
}

pub fn flatten_comments(children: &[Value], depth: u32, out: &mut CommentTree) {
    for child in children {
        let kind = s(child, "kind").unwrap_or("");
        let d = match child.get("data") {
            Some(d) => d,
            None => continue,
        };
        match kind {
            "t1" => {
                out.comments.push(comment_from(d, depth));
                let replies = d.get("replies");
                if let Some(Value::Object(replies)) = replies
                    && let Some(rc) = replies
                        .get("data")
                        .and_then(|x| x.get("children"))
                        .and_then(|x| x.as_array())
                {
                    flatten_comments(rc, depth + 1, out);
                }
            }
            "more" => {
                let ids = d
                    .get("children")
                    .and_then(|x| x.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                if let Some(id) = so(d, "name") {
                    out.more_children.push(id);
                }
                // A "more" at depth d stands for additional children of the
                // ancestor comment at depth d-1.
                if depth > 0 {
                    let owner_depth = depth - 1;
                    if let Some(c) = out
                        .comments
                        .iter_mut()
                        .rev()
                        .find(|c| c.depth == owner_depth)
                    {
                        c.more_children_count += ids as u64;
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct FeedResult {
    pub posts: Vec<PostSummary>,
    /// Opaque cursor: pass as `after` to fetch the next page.
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct PostDetail {
    pub post: Option<PostSummary>,
    pub comments: CommentTree,
    pub comment_sort: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SubredditAbout {
    /// Fullname (t5_...)
    pub id: Option<String>,
    pub display_name: String,
    pub display_name_prefixed: Option<String>,
    pub title: Option<String>,
    pub subscribers: Option<i64>,
    pub active_user_count: Option<i64>,
    pub created_utc: Option<f64>,
    pub over18: Option<bool>,
    pub description: TruncatedText,
    pub submission_type: Option<String>,
    pub url: Option<String>,
    pub user_is_banned: Option<bool>,
    pub user_is_moderator: Option<bool>,
    pub user_is_subscriber: Option<bool>,
}

pub fn subreddit_from(d: &Value) -> SubredditAbout {
    SubredditAbout {
        id: so(d, "name"),
        display_name: so(d, "display_name").unwrap_or_default(),
        display_name_prefixed: so(d, "display_name_prefixed"),
        title: so(d, "title"),
        subscribers: n(d, "subscribers"),
        active_user_count: n(d, "active_user_count"),
        created_utc: f(d, "created_utc"),
        over18: b(d, "over18"),
        description: truncate(
            s(d, "public_description")
                .filter(|x| !x.is_empty())
                .unwrap_or_default(),
            MAX_TEXT,
        ),
        submission_type: so(d, "submission_type"),
        url: absolutize(so(d, "url")),
        user_is_banned: b(d, "user_is_banned"),
        user_is_moderator: b(d, "user_is_moderator"),
        user_is_subscriber: b(d, "user_is_subscriber"),
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SubredditRule {
    pub short_name: String,
    pub description: TruncatedText,
    pub violation_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SubredditDetail {
    pub about: Option<SubredditAbout>,
    pub rules: Vec<SubredditRule>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct UserDetail {
    pub about: Option<Identity>,
    pub posts: Vec<PostSummary>,
    pub comments: Vec<CommentSummary>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct MessageSummary {
    /// Fullname (t4_...)
    pub id: String,
    pub author: Option<String>,
    pub dest: Option<String>,
    pub subject: Option<String>,
    pub body: TruncatedText,
    pub created_utc: Option<f64>,
    pub new: Option<bool>,
    /// Set when the item is a comment reply rather than a private message.
    pub was_comment: Option<bool>,
    pub link_title: Option<String>,
    pub context: Option<String>,
    pub subreddit: Option<String>,
}

pub fn message_from(d: &Value) -> MessageSummary {
    MessageSummary {
        id: so(d, "name").unwrap_or_default(),
        author: so(d, "author"),
        dest: so(d, "dest"),
        subject: so(d, "subject"),
        body: truncate(s(d, "body").unwrap_or_default(), MAX_TEXT),
        created_utc: f(d, "created_utc"),
        new: b(d, "new"),
        was_comment: b(d, "was_comment"),
        link_title: so(d, "link_title"),
        context: so(d, "context").map(|c| format!("https://reddit.com{c}")),
        subreddit: so(d, "subreddit"),
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct InboxResult {
    pub messages: Vec<MessageSummary>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SearchResult {
    pub posts: Vec<PostSummary>,
    pub subreddits: Vec<SubredditAbout>,
    pub users: Vec<String>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ModQueueResult {
    pub posts: Vec<PostSummary>,
    pub comments: Vec<CommentSummary>,
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ModLogEntry {
    pub action: Option<String>,
    pub mod_name: Option<String>,
    pub created_utc: Option<f64>,
    pub target_author: Option<String>,
    pub target_title: Option<String>,
    pub target_permalink: Option<String>,
    pub target_body: Option<String>,
    pub details: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ModUserEntry {
    pub name: String,
    pub id: Option<String>,
    pub since_utc: Option<f64>,
    pub note: Option<String>,
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct FlairOption {
    pub id: Option<String>,
    pub text: Option<String>,
    pub editable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AuthStatus {
    pub configured: bool,
    pub preset: String,
    pub redirect_uri: String,
    pub user_agent: String,
    pub client_id: String,
    pub logged_in: bool,
    pub username: Option<String>,
    pub scopes: Option<String>,
    pub access_token_expires_utc: Option<i64>,
    pub identity: Option<Identity>,
    /// Which TLS stack is active, e.g. "native-tls (primary)" or
    /// "rustls (failover active)".
    pub tls_stack: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct LoginStart {
    pub authorize_url: String,
    pub redirect_uri: String,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ActionResult {
    pub ok: bool,
    pub detail: String,
    /// The created/affected thing, when Reddit returns one.
    pub created_post: Option<PostSummary>,
    pub created_comment: Option<CommentSummary>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ModmailConversation {
    pub id: String,
    pub subject: Option<String>,
    pub state: Option<String>,
    /// Owning subreddit.
    pub owner: Option<String>,
    pub authors: Vec<String>,
    pub num_messages: Option<i64>,
    pub last_updated_utc: Option<f64>,
    pub is_internal: Option<bool>,
    pub is_unread: Option<bool>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ModmailMessage {
    pub id: String,
    pub author: Option<String>,
    pub author_is_hidden: Option<bool>,
    pub body: TruncatedText,
    pub created_utc: Option<f64>,
    pub is_internal: Option<bool>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ModmailThread {
    pub conversation: Option<ModmailConversation>,
    pub messages: Vec<ModmailMessage>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ModmailList {
    pub conversations: Vec<ModmailConversation>,
}

// ---------------------------------------------------------------------------
// Listing helpers
// ---------------------------------------------------------------------------

pub struct Listing {
    pub children: Vec<Value>,
    pub after: Option<String>,
}

pub fn listing(v: &Value) -> Listing {
    let empty = Vec::new();
    let children = v
        .get("data")
        .and_then(|d| d.get("children"))
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or(empty);
    let after = v.get("data").and_then(|d| so(d, "after"));
    Listing { children, after }
}

pub fn posts_from_listing(v: &Value) -> FeedResult {
    let l = listing(v);
    FeedResult {
        posts: l
            .children
            .iter()
            .filter(|c| s(c, "kind") == Some("t3"))
            .filter_map(|c| c.get("data"))
            .map(post_from)
            .collect(),
        next_after: l.after,
    }
}

pub fn mixed_from_listing(v: &Value) -> ModQueueResult {
    let l = listing(v);
    let mut posts = Vec::new();
    let mut comments = Vec::new();
    for c in &l.children {
        match s(c, "kind") {
            Some("t3") => {
                if let Some(d) = c.get("data") {
                    posts.push(post_from(d));
                }
            }
            Some("t1") => {
                if let Some(d) = c.get("data") {
                    comments.push(comment_from(d, 0));
                }
            }
            _ => {}
        }
    }
    ModQueueResult {
        posts,
        comments,
        next_after: l.after,
    }
}

pub fn messages_from_listing(v: &Value) -> InboxResult {
    let l = listing(v);
    InboxResult {
        messages: l
            .children
            .iter()
            .filter_map(|c| c.get("data"))
            .map(message_from)
            .collect(),
        next_after: l.after,
    }
}

pub fn modlog_from_listing(v: &Value) -> Vec<ModLogEntry> {
    listing(v)
        .children
        .iter()
        .filter(|c| s(c, "kind") == Some("modaction"))
        .filter_map(|c| c.get("data"))
        .map(|d| ModLogEntry {
            action: so(d, "action"),
            mod_name: so(d, "mod"),
            created_utc: f(d, "created_utc"),
            target_author: so(d, "target_author"),
            target_title: so(d, "target_title"),
            target_permalink: absolutize(so(d, "target_permalink")),
            target_body: so(d, "target_body").map(|x| truncate(&x, 500).text),
            details: so(d, "details"),
            description: so(d, "description"),
        })
        .collect()
}

pub fn modusers_from_listing(v: &Value) -> Vec<ModUserEntry> {
    listing(v)
        .children
        .iter()
        .filter_map(|c| c.get("data"))
        .map(|d| ModUserEntry {
            name: so(d, "name").unwrap_or_default(),
            id: so(d, "id"),
            since_utc: f(d, "date"),
            note: so(d, "note"),
            permissions: d
                .get("mod_permissions")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.as_str().map(|x| x.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

pub fn search_from_listing(v: &Value) -> SearchResult {
    let l = listing(v);
    let mut posts = Vec::new();
    let mut subreddits = Vec::new();
    let mut users = Vec::new();
    for c in &l.children {
        match s(c, "kind") {
            Some("t3") => {
                if let Some(d) = c.get("data") {
                    posts.push(post_from(d));
                }
            }
            Some("t5") => {
                if let Some(d) = c.get("data") {
                    subreddits.push(subreddit_from(d));
                }
            }
            Some("t2") => {
                if let Some(d) = c.get("data")
                    && let Some(name) = so(d, "name")
                {
                    users.push(name);
                }
            }
            _ => {}
        }
    }
    SearchResult {
        posts,
        subreddits,
        users,
        next_after: l.after,
    }
}

/// Best-effort parse of a new-modmail conversation object.
pub fn modmail_conversation_from(id: &str, c: &Value) -> ModmailConversation {
    let mut authors: Vec<String> = Vec::new();
    if let Some(arr) = c.get("authors").and_then(|x| x.as_array()) {
        for a in arr {
            if let Some(name) = a.as_str() {
                let name = name
                    .strip_prefix("u_")
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| name.to_string());
                authors.push(name);
            }
        }
    } else if let Some(parts) = c.get("participantIds").and_then(|x| x.as_array()) {
        for p in parts {
            if let Some(pid) = p.as_str() {
                authors.push(pid.to_string());
            }
        }
    }
    let owner = c
        .get("owner")
        .and_then(|o| so(o, "displayName").or_else(|| so(o, "id")))
        .map(|o| o.trim_start_matches("t5_").to_string());
    ModmailConversation {
        id: id.to_string(),
        subject: so(c, "subject"),
        state: so(c, "state"),
        owner,
        authors,
        num_messages: n(c, "numMessages"),
        // modmail timestamps are epoch milliseconds
        last_updated_utc: f(c, "lastUpdated").map(|x| x / 1000.0),
        is_internal: b(c, "isInternal"),
        is_unread: b(c, "isUnread"),
    }
}

pub fn modmail_thread_from(v: &Value) -> ModmailThread {
    let conversation = v
        .get("conversation")
        .and_then(|c| so(c, "id"))
        .map(|id| modmail_conversation_from(&id, v.get("conversation").unwrap_or(&Value::Null)));
    let mut messages = Vec::new();
    if let Some(map) = v.get("messages").and_then(|m| m.as_object()) {
        for (_k, m) in map {
            let author = m
                .get("author")
                .and_then(|a| so(a, "name").or_else(|| so(a, "id")));
            messages.push(ModmailMessage {
                id: so(m, "id").unwrap_or_default(),
                author,
                author_is_hidden: m.get("author").and_then(|a| b(a, "isHidden")),
                body: truncate(s(m, "bodyMarkdown").unwrap_or_default(), MAX_TEXT),
                created_utc: f(m, "created").map(|x| if x > 1e12 { x / 1000.0 } else { x }),
                is_internal: b(m, "isInternal"),
            });
        }
    }
    messages.sort_by_key(|m| m.created_utc.map(|x| (x * 1000.0) as i64).unwrap_or(0));
    ModmailThread {
        conversation,
        messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncates_long_text() {
        let t = truncate(&"x".repeat(2500), MAX_TEXT);
        assert!(t.truncated);
        assert!(t.text.ends_with("[truncated]"));
        assert!(t.text.chars().count() < 2100);
    }

    #[test]
    fn parses_listing() {
        let v = json!({"kind":"Listing","data":{"after":"t3_abc","children":[
            {"kind":"t3","data":{"name":"t3_1","subreddit":"rust","title":"hello","selftext":"body"}},
            {"kind":"t5","data":{"display_name":"rust"}}
        ]}});
        let l = listing(&v);
        assert_eq!(l.children.len(), 2);
        assert_eq!(l.after.as_deref(), Some("t3_abc"));
        let feed = posts_from_listing(&v);
        assert_eq!(feed.posts.len(), 1);
        assert_eq!(feed.posts[0].id, "t3_1");
        assert!(!feed.posts[0].title.truncated);
    }

    #[test]
    fn flattens_comment_tree() {
        let v = json!({"data":{"children":[
            {"kind":"t1","data":{"name":"t1_a","body":"top","replies":{"data":{"children":[
                {"kind":"t1","data":{"name":"t1_b","body":"nested","replies":""}},
                {"kind":"more","data":{"name":"t1_x","children":["c1","c2"]}}
            ]}}}},
            {"kind":"t1","data":{"name":"t1_c","body":"second","replies":""}}
        ]}});
        let mut tree = CommentTree::default();
        let children = v["data"]["children"].as_array().unwrap().clone();
        flatten_comments(&children, 0, &mut tree);
        assert_eq!(tree.comments.len(), 3);
        assert_eq!(tree.comments[0].depth, 0);
        assert_eq!(tree.comments[1].depth, 1);
        assert_eq!(tree.comments[1].parent_id.as_deref(), None);
        assert_eq!(tree.more_children.len(), 1);
        assert_eq!(tree.comments[0].more_children_count, 2);
    }

    #[test]
    fn formats_modmail_thread_sorted() {
        let v = json!({
            "conversation": {"id":"abc", "subject":"s", "authors":["u_bob","t5_rust"], "owner": {"id":"t5_rust"}, "lastUpdated": 1700000000000u64},
            "messages": {
                "m2": {"id":"m2","author":{"name":"alice"},"bodyMarkdown":"second","created":2000.0},
                "m1": {"id":"m1","author":{"name":"bob","isHidden":false},"bodyMarkdown":"first","created":1000.0}
            }
        });
        let t = modmail_thread_from(&v);
        assert_eq!(t.messages.len(), 2);
        assert_eq!(t.messages[0].id, "m1");
        let c = t.conversation.unwrap();
        assert_eq!(c.authors, vec!["bob".to_string(), "t5_rust".to_string()]);
        assert_eq!(c.owner.as_deref(), Some("rust"));
    }
}
