use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

use super::{check_response, GistDetail, GistFile, GistProvider, GistSummary, ProviderError, UserInfo};

/// GitHub Gist REST API (v3) 实现
pub struct GithubProvider {
    token: String,
    base_url: String,
    client: reqwest::Client,
}

impl GithubProvider {
    pub fn new(token: String) -> Self {
        Self::new_with_base(token, "https://api.github.com".to_string())
    }

    /// 测试注入 mock server 地址
    pub fn new_with_base(token: String, base_url: String) -> Self {
        Self {
            token,
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    fn request(
        &self,
        method: reqwest::Method,
        url: String,
        body: Option<serde_json::Value>,
    ) -> reqwest::RequestBuilder {
        let mut rb = self
            .client
            .request(method, &url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "gist-auto-sync/0.1.0");
        if let Some(body) = body {
            rb = rb.json(&body);
        }
        rb
    }
}

/// GitHub API 响应结构（内部解析用）
#[derive(Debug, Deserialize)]
struct GhGist {
    id: String,
    description: Option<String>,
    updated_at: DateTime<Utc>,
    files: HashMap<String, GhFile>,
}

#[derive(Debug, Deserialize)]
struct GhFile {
    #[serde(default)]
    filename: Option<String>,
    raw_url: Option<String>,
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
    name: Option<String>,
}

fn to_summary(g: GhGist) -> GistSummary {
    GistSummary {
        id: g.id,
        description: g.description,
        updated_at: g.updated_at,
        files: g
            .files
            .into_iter()
            .map(|(key, f)| f.filename.unwrap_or(key))
            .collect(),
    }
}

fn to_detail(g: GhGist) -> GistDetail {
    GistDetail {
        id: g.id,
        description: g.description,
        updated_at: g.updated_at,
        files: g
            .files
            .into_iter()
            .map(|(key, f)| GistFile {
                filename: f.filename.unwrap_or(key),
                raw_url: f.raw_url,
                content: f.content,
            })
            .collect(),
    }
}

#[async_trait::async_trait]
impl GistProvider for GithubProvider {
    async fn verify_token(&self) -> Result<UserInfo, ProviderError> {
        let resp = self
            .request(reqwest::Method::GET, format!("{}/user", self.base_url), None)
            .send()
            .await?;
        let user: GhUser = check_response(resp).await?.json().await?;
        Ok(UserInfo {
            login: user.login,
            name: user.name,
        })
    }

    async fn list_gists(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<GistSummary>, ProviderError> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut page: u32 = 1;
        loop {
            let mut url = format!("{}/gists?per_page=100&page={}", self.base_url, page);
            if let Some(since) = since {
                // 用 Z 后缀的 RFC3339，避免 +00:00 在 URL 查询串中被百分号编码
                url.push_str(&format!(
                    "&since={}",
                    since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                ));
            }
            let resp = self
                .request(reqwest::Method::GET, url, None)
                .send()
                .await?;
            let gists: Vec<GhGist> = check_response(resp).await?.json().await?;
            let is_full_page = gists.len() == 100;
            for g in gists {
                if seen.insert(g.id.clone()) {
                    out.push(to_summary(g));
                }
            }
            if !is_full_page {
                break;
            }
            if page >= super::MAX_LIST_PAGES {
                tracing::warn!(
                    "list_gists 分页达安全上限 {} 页，提前停止（服务端疑似异常）",
                    super::MAX_LIST_PAGES
                );
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    async fn get_gist(
        &self,
        gist_id: &str,
        wanted_file: Option<&str>,
    ) -> Result<GistDetail, ProviderError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                format!("{}/gists/{gist_id}", self.base_url),
                None,
            )
            .send()
            .await?;
        let g = check_response(resp).await?.json::<GhGist>().await?;
        let mut detail = to_detail(g);
        self.backfill_raw_content(&mut detail, wanted_file).await?;
        Ok(detail)
    }

    async fn update_file(
        &self,
        gist_id: &str,
        filename: &str,
        content: &str,
    ) -> Result<GistDetail, ProviderError> {
        let body = json!({ "files": { filename: { "content": content } } });
        let resp = self
            .request(
                reqwest::Method::PATCH,
                format!("{}/gists/{gist_id}", self.base_url),
                Some(body),
            )
            .send()
            .await?;
        let g = check_response(resp).await?.json::<GhGist>().await?;
        let mut detail = to_detail(g);
        // PATCH 响应只关心被改的文件（回读校验用）
        self.backfill_raw_content(&mut detail, Some(filename)).await?;
        Ok(detail)
    }
}

impl GithubProvider {
    /// 详情响应 content 缺失（大文件被置 null）时，经 raw_url 回补。
    /// `wanted_file`：只回补该文件，避免为 gist 内无关文件放大流量；None 全部回补。
    async fn backfill_raw_content(
        &self,
        detail: &mut GistDetail,
        wanted_file: Option<&str>,
    ) -> Result<(), ProviderError> {
        for f in detail.files.iter_mut() {
            if f.content.is_some() {
                continue;
            }
            if let Some(w) = wanted_file
                && f.filename != w
            {
                continue;
            }
            let Some(raw_url) = f.raw_url.clone() else {
                continue;
            };
            let resp = self
                .request(reqwest::Method::GET, raw_url, None)
                .send()
                .await?;
            f.content = Some(check_response(resp).await?.text().await?);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn provider(server: &MockServer) -> GithubProvider {
        GithubProvider::new_with_base("ghp_test_token".into(), server.uri())
    }

    fn gist_json(
        id: &str,
        description: &str,
        filename: &str,
        content: Option<&str>,
        raw_url: Option<String>,
    ) -> serde_json::Value {
        json!({
            "id": id,
            "description": description,
            "updated_at": "2026-10-13T08:00:00Z",
            "files": {
                filename: {
                    "filename": filename,
                    "raw_url": raw_url,
                    "content": content,
                    "size": content.map(|c| c.len()).unwrap_or(0),
                    "truncated": false
                }
            }
        })
    }

    #[tokio::test]
    async fn list_gists_stops_at_max_pages_on_runaway_server() {
        // 异常服务端恒返满页：分页循环必须在安全上限处停止（防死循环）
        let server = MockServer::start().await;
        let full_page: Vec<_> = (0..100)
            .map(|i| gist_json(&format!("g{i}"), "d", "a.txt", Some("x"), None))
            .collect();
        Mock::given(method("GET"))
            .and(path("/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(full_page)))
            .expect(crate::provider::MAX_LIST_PAGES as u64)
            .mount(&server)
            .await;
        let out = provider(&server).list_gists(None).await.unwrap();
        assert_eq!(out.len(), 100, "同页内容去重后应恰为 100 条");
        server.verify().await; // 请求数恰好停在安全上限
    }

    #[tokio::test]
    async fn verify_token_sends_bearer_and_version_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer ghp_test_token",
            ))
            .and(wiremock::matchers::header(
                "x-github-api-version",
                "2022-11-28",
            ))
            .and(wiremock::matchers::header_regex("user-agent", r".+"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"login": "octocat", "name": "The Octocat"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let user = provider(&server).verify_token().await.unwrap();
        assert_eq!(user.login, "octocat");
        assert_eq!(user.name.as_deref(), Some("The Octocat"));
    }

    #[tokio::test]
    async fn list_gists_paginates_and_forwards_since() {
        let server = MockServer::start().await;
        let page1: Vec<_> = (0..100)
            .map(|i| gist_json(&format!("g{i}"), "d", "a.txt", Some("x"), None))
            .collect();
        Mock::given(method("GET"))
            .and(path("/gists"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .and(query_param("since", "2026-10-01T00:00:00Z"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(page1)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/gists"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                gist_json("g-last", "最终", "b.md", Some("y"), None)
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let gists = provider(&server)
            .list_gists(Some(chrono::Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()))
            .await
            .unwrap();
        assert_eq!(gists.len(), 101);
        assert_eq!(gists[0].id, "g0");
        assert_eq!(gists[100].id, "g-last");
        assert_eq!(gists[100].files, vec!["b.md".to_string()]);
    }

    #[tokio::test]
    async fn get_gist_parses_files_and_backfills_raw_content() {
        let server = MockServer::start().await;
        let raw_url = format!("{}/raw/gist1/notes.md", server.uri());
        Mock::given(method("GET"))
            .and(path("/gists/gist1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json(
                "gist1",
                "笔记",
                "notes.md",
                None,
                Some(raw_url.clone()),
            )))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/raw/gist1/notes.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("raw content here"))
            .expect(1)
            .mount(&server)
            .await;

        let detail = provider(&server).get_gist("gist1", Some("notes.md")).await.unwrap();
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].filename, "notes.md");
        assert_eq!(detail.files[0].content.as_deref(), Some("raw content here"));
    }

    #[tokio::test]
    async fn get_gist_backfills_only_wanted_file() {
        // 双文件 gist：只有目标文件回补 raw，无关文件即使缺 content 也不拉（流量不放大）
        let server = MockServer::start().await;
        let raw_url = format!("{}/raw/gist1/notes.md", server.uri());
        let body = json!({
            "id": "gist1",
            "description": "双文件",
            "updated_at": "2026-10-13T08:00:00Z",
            "files": {
                "notes.md": { "filename": "notes.md", "raw_url": raw_url, "content": null, "size": 0, "truncated": false },
                "other.md": { "filename": "other.md", "raw_url": null, "content": null, "size": 0, "truncated": false }
            }
        });
        Mock::given(method("GET"))
            .and(path("/gists/gist1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/raw/gist1/notes.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("raw content here"))
            .expect(1)
            .mount(&server)
            .await;

        let detail = provider(&server).get_gist("gist1", Some("notes.md")).await.unwrap();
        // files 来自 HashMap，顺序不定：按文件名断言
        let wanted = detail.files.iter().find(|f| f.filename == "notes.md").unwrap();
        assert_eq!(wanted.content.as_deref(), Some("raw content here"));
        let other = detail.files.iter().find(|f| f.filename == "other.md").unwrap();
        assert!(other.content.is_none(), "无关文件不应被回补");
        server.verify().await;
    }

    #[tokio::test]
    async fn update_file_patches_single_file_key_only() {
        let server = MockServer::start().await;
        let expected_body = json!({
            "files": { "notes.md": { "content": "new content" } }
        });
        Mock::given(method("PATCH"))
            .and(path("/gists/gist1"))
            .and(body_json(expected_body))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json(
                "gist1",
                "笔记",
                "notes.md",
                Some("new content"),
                None,
            )))
            .expect(1)
            .mount(&server)
            .await;

        let detail = provider(&server)
            .update_file("gist1", "notes.md", "new content")
            .await
            .unwrap();
        assert_eq!(detail.files[0].content.as_deref(), Some("new content"));
    }

    // ── 终审修复：GitHub primary rate limit（403 + X-RateLimit 头）应映射 RateLimited ──
    #[tokio::test]
    async fn github_primary_rate_limit_403_maps_to_rate_limited() {
        let server = MockServer::start().await;
        let reset = chrono::Utc::now().timestamp() + 120;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("x-ratelimit-reset", reset.to_string())
                    .set_body_string("rate limit exceeded"),
            )
            .mount(&server)
            .await;
        match provider(&server).verify_token().await.unwrap_err() {
            ProviderError::RateLimited { retry_after: Some(d) } => {
                let secs = d.as_secs();
                assert!((90..=140).contains(&secs), "reset 距今约 120s，实际 {secs}");
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn error_401_maps_to_auth() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(401).set_body_string("{\"message\":\"Bad credentials\"}"),
            )
            .mount(&server)
            .await;
        let err = provider(&server).verify_token().await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth), "got: {err:?}");
    }

    #[tokio::test]
    async fn error_429_maps_to_rate_limited_with_retry_after() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "30")
                    .set_body_string("rate limit exceeded"),
            )
            .mount(&server)
            .await;
        match provider(&server).verify_token().await.unwrap_err() {
            ProviderError::RateLimited {
                retry_after: Some(d),
            } => assert_eq!(d, std::time::Duration::from_secs(30)),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }
}
