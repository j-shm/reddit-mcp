//! Integration tests for the Reddit client against a mock Reddit, covering the
//! OAuth flows (manual-paste code exchange, refresh-on-401), error-payload
//! translation, and 429 backoff.

use std::path::PathBuf;

use reddit_mcp::client::{Base, RedditClient, RedditError};
use reddit_mcp::config::Config;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_cfg(server: &MockServer, data_dir: PathBuf) -> Config {
    Config {
        client_id: "testclient".to_string(),
        redirect_uri: "http://localhost:8080".to_string(),
        user_agent: "test:reddit-mcp:v0.1 (by /u/tester)".to_string(),
        www_base: server.uri(),
        oauth_base: server.uri(),
        mod_base: server.uri(),
        data_dir: Some(data_dir),
        ..Config::default()
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("reddit-mcp-test-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn token_body(access: &str, refresh: Option<&str>) -> String {
    let refresh = refresh
        .map(|r| format!(",\"refresh_token\":\"{r}\""))
        .unwrap_or_default();
    format!(
        "{{\"access_token\":\"{access}\",\"token_type\":\"bearer\",\"expires_in\":3600,\"scope\":\"*\"{refresh}}}"
    )
}

fn me_body() -> String {
    "{\"name\":\"tester\",\"link_karma\":10,\"comment_karma\":20,\"is_mod\":true}".to_string()
}

async fn logged_in_client(server: &MockServer, dir: PathBuf) -> RedditClient {
    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(200).set_body_string(token_body("AT1", Some("RT1"))))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_string(me_body()))
        .mount(server)
        .await;
    let client = RedditClient::new(test_cfg(server, dir)).unwrap();
    client
        .login_complete("dystopia://response?code=THECODE")
        .await
        .unwrap();
    client
}

#[tokio::test]
async fn login_flow_exchanges_code_and_persists_tokens() {
    let server = MockServer::start().await;
    let dir = temp_dir("login");
    let client = RedditClient::new(test_cfg(&server, dir.clone())).unwrap();

    let start = client.login_start().await.unwrap();
    assert!(start.authorize_url.contains("duration=permanent"));
    assert!(start.authorize_url.contains("response_type=code"));
    assert!(
        start
            .authorize_url
            .contains("redirect_uri=http%3A%2F%2Flocalhost%3A8080")
    );
    assert_eq!(start.steps.len(), 5);

    let client2 = logged_in_client(&server, dir.clone()).await;
    let _ = client2;
    // logged_in_client already exchanged the code; verify persistence.
    let store: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("tokens.json")).unwrap()).unwrap();
    assert_eq!(store["refresh_token"], "RT1");
    assert_eq!(store["username"], "tester");
    assert_eq!(store["access_token"], "AT1");
}

#[tokio::test]
async fn get_refreshes_token_on_401_and_retries() {
    let server = MockServer::start().await;
    let client = logged_in_client(&server, temp_dir("refresh")).await;

    // Stale AT1 gets 401; the refreshed AT2 bearer succeeds.
    Mock::given(method("GET"))
        .and(path("/hot"))
        .and(header("authorization", "Bearer AT1"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hot"))
        .and(header("authorization", "Bearer AT2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"kind":"Listing","data":{"after":null,"children":[]}}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(token_body("AT2", None)))
        .expect(1)
        .mount(&server)
        .await;

    let v = client.get(Base::Oauth, true, "/hot", &[]).await.unwrap();
    assert_eq!(v["kind"], "Listing");
}

#[tokio::test]
async fn api_errors_return_actionable_messages() {
    let server = MockServer::start().await;
    let client = logged_in_client(&server, temp_dir("apierror")).await;

    Mock::given(method("GET"))
        .and(path("/hot"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"json":{"errors":[["ALREADY_SUB","that link has already been submitted","url"]]}}"#,
        ))
        .mount(&server)
        .await;

    let err = client
        .get(Base::Oauth, true, "/hot", &[])
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("ALREADY_SUB"), "got: {msg}");
    assert!(msg.contains("resubmit=true"), "got: {msg}");
}

#[tokio::test]
async fn revoked_refresh_token_reports_relogin() {
    let server = MockServer::start().await;
    let dir = temp_dir("revoked");
    let _ = logged_in_client(&server, dir.clone()).await;

    // Expire the cached access token so the next call must refresh.
    let tokens_path = dir.join("tokens.json");
    let mut store: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&tokens_path).unwrap()).unwrap();
    store["expires_at"] = serde_json::json!(0);
    std::fs::write(&tokens_path, serde_json::to_string(&store).unwrap()).unwrap();
    let client2 = RedditClient::new(test_cfg(&server, dir)).unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_grant"}"#))
        .mount(&server)
        .await;

    let err = client2.identity().await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("reddit_login_start"), "got: {msg}");
}

#[tokio::test]
async fn backoff_on_429_with_retry_after() {
    let server = MockServer::start().await;
    let client = RedditClient::new(test_cfg(&server, temp_dir("ratelimit"))).unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(token_body("APP", None)))
        .mount(&server)
        .await;
    // wiremock matches the first-mounted mock, so the 429 (limited to one
    // hit) must be mounted before the success response.
    Mock::given(method("GET"))
        .and(path("/hot"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "1"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hot"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"kind":"Listing","data":{"after":null,"children":[]}}"#),
        )
        .mount(&server)
        .await;

    let started = std::time::Instant::now();
    let v = client.get(Base::Oauth, false, "/hot", &[]).await.unwrap();
    assert_eq!(v["kind"], "Listing");
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
}

#[tokio::test]
async fn submit_parses_created_thing() {
    let server = MockServer::start().await;
    let client = logged_in_client(&server, temp_dir("submit")).await;

    Mock::given(method("POST"))
        .and(path("/api/submit"))
        .and(body_string_contains("sr=rust"))
        .and(body_string_contains("kind=self"))
        .and(body_string_contains("api_type=json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"json":{"errors":[],"data":{"things":[{"kind":"t3","data":{"name":"t3_abc","subreddit":"rust","title":"hello","selftext":"body","permalink":"/r/rust/comments/abc/hello/"}}]}}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let v = client
        .post(
            "/api/submit",
            vec![
                ("sr".into(), "rust".into()),
                ("kind".into(), "self".into()),
                ("title".into(), "hello".into()),
                ("text".into(), "body".into()),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        v.pointer("/json/data/things/0/data/name")
            .and_then(|x| x.as_str()),
        Some("t3_abc")
    );
}

#[tokio::test]
async fn not_logged_in_error_for_user_actions() {
    let server = MockServer::start().await;
    let client = RedditClient::new(test_cfg(&server, temp_dir("nologin"))).unwrap();
    let err = client.identity().await.unwrap_err();
    assert!(err.to_string().contains("reddit_login_start"));
}

#[tokio::test]
async fn retries_transient_html_403_block_pages() {
    let server = MockServer::start().await;
    let client = RedditClient::new(test_cfg(&server, temp_dir("blockpage"))).unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(token_body("APP", None)))
        .mount(&server)
        .await;
    // wiremock matches the first-mounted mock; limit the HTML block page to
    // one hit so the retry reaches the success mock.
    Mock::given(method("GET"))
        .and(path("/hot"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_string("<body class=theme-beta><div>blocked</div></body>"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hot"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"kind":"Listing","data":{"after":null,"children":[]}}"#),
        )
        .mount(&server)
        .await;

    let v = client.get(Base::Oauth, false, "/hot", &[]).await.unwrap();
    assert_eq!(v["kind"], "Listing");
}

#[tokio::test]
async fn token_endpoint_retries_html_403_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(403).set_body_string("<html>blocked</html>"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(ResponseTemplate::new(200).set_body_string(token_body("AT1", Some("RT1"))))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_string(me_body()))
        .mount(&server)
        .await;

    let client = RedditClient::new(test_cfg(&server, temp_dir("tokenblock"))).unwrap();
    let me = client
        .login_complete("dystopia://response?code=THECODE")
        .await
        .unwrap();
    assert_eq!(me.name, "tester");
}

#[tokio::test]
async fn json_403_fails_immediately_without_retry() {
    let server = MockServer::start().await;
    let client = RedditClient::new(test_cfg(&server, temp_dir("json403"))).unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(token_body("APP", None)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hot"))
        .respond_with(
            ResponseTemplate::new(403).set_body_string(r#"{"message":"Forbidden","error":403}"#),
        )
        .mount(&server)
        .await;

    let err = client
        .get(Base::Oauth, false, "/hot", &[])
        .await
        .unwrap_err();
    match err {
        RedditError::Http { status, .. } => assert_eq!(status, 403),
        other => panic!("expected Http(403), got: {other}"),
    }
}

#[tokio::test]
async fn switches_tls_stack_after_repeated_block_pages() {
    let server = MockServer::start().await;
    let client = RedditClient::new(test_cfg(&server, temp_dir("failover"))).unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(token_body("APP", None)))
        .mount(&server)
        .await;
    // Two block pages, then success: first retries the same stack, the
    // second should flip the failover switch and retry immediately.
    Mock::given(method("GET"))
        .and(path("/hot"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_string("<body class=theme-beta><div>blocked</div></body>"),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hot"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"kind":"Listing","data":{"after":null,"children":[]}}"#),
        )
        .mount(&server)
        .await;

    let v = client.get(Base::Oauth, false, "/hot", &[]).await.unwrap();
    assert_eq!(v["kind"], "Listing");
    assert!(
        client.failover_active(),
        "failover stack should be engaged after two block pages"
    );
}

#[tokio::test]
async fn persistent_block_pages_surface_actionable_error() {
    let server = MockServer::start().await;
    let client = RedditClient::new(test_cfg(&server, temp_dir("blocked"))).unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(token_body("APP", None)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/hot"))
        .respond_with(ResponseTemplate::new(403).set_body_string("<html>blocked</html>"))
        .mount(&server)
        .await;

    let err = client
        .get(Base::Oauth, false, "/hot", &[])
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("TLS"), "got: {msg}");
    assert!(msg.contains("backend"), "got: {msg}");
}
