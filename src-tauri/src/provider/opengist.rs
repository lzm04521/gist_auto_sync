use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

use super::{
    check_response, GistCommit, GistDetail, GistFile, GistProvider, GistSummary, ProviderError,
    UserInfo,
};

/// Opengist REST API 实现。
/// 端点映射（核对自 https://opengist.io/docs/api ，v1.15.2）：
/// - 认证用户   GET   {base}/api/user
/// - 列 gists   GET   {base}/api/gists?page&per_page&since(RFC3339)
/// - gist 详情  GET   {base}/api/gists/{uuid}
/// - 更新 gist  PATCH {base}/api/gists/{uuid}  body: GistUpdateRequest（files map 与 GitHub 同构）
/// - raw 文件   GET   {base}/api/gists/{uuid}/files/{sha}/{filename}（sha 用详情响应 commits[0].version）
///
/// 认证：`Authorization: Bearer og_xxx`；scope `gist:read`/`gist:write`。
pub struct OpengistProvider {
    base_url: String,
    token: String,
    client: reqwest::Client,
}

impl OpengistProvider {
    pub fn new(base_url: String, token: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
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
            .header("Accept", "application/json")
            .header("User-Agent", "gist-auto-sync/0.1.0");
        if let Some(body) = body {
            rb = rb.json(&body);
        }
        rb
    }
}

/// Opengist 响应结构（内部解析用）。
/// 列表项（GistSimple）无 files/commits 字段 → 用 serde default 兼容。
#[derive(Debug, Deserialize)]
struct OgGist {
    id: String,
    #[serde(default)]
    description: Option<String>,
    /// GistSimple/Gist 均带 title；缺失时容忍为 None
    #[serde(default)]
    title: Option<String>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    files: HashMap<String, OgFile>,
    #[serde(default)]
    commits: Vec<OgCommit>,
}

#[derive(Debug, Deserialize)]
struct OgFile {
    #[serde(default)]
    filename: Option<String>,
    content: Option<String>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct OgCommit {
    version: String,
}

/// GET /api/gists/{uuid}/commits 响应项（openapi.yaml GistCommit）
#[derive(Debug, Deserialize)]
struct OgCommitEntry {
    version: String,
    committed_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct OgUser {
    login: String,
    #[serde(default)]
    username: Option<String>,
}

fn to_summary(g: OgGist) -> GistSummary {
    GistSummary {
        id: g.id,
        description: g.description,
        title: g.title,
        updated_at: g.updated_at,
        files: g
            .files
            .into_iter()
            .map(|(key, f)| f.filename.unwrap_or(key))
            .collect(),
    }
}

fn to_detail(g: OgGist) -> (GistDetail, Option<String>) {
    let sha = g.commits.first().map(|c| c.version.clone());
    let detail = GistDetail {
        id: g.id,
        description: g.description,
        updated_at: g.updated_at,
        files: g
            .files
            .into_iter()
            .map(|(key, f)| GistFile {
                filename: f.filename.unwrap_or(key),
                raw_url: None,
                content: if f.truncated { None } else { f.content },
            })
            .collect(),
    };
    (detail, sha)
}

#[async_trait::async_trait]
impl GistProvider for OpengistProvider {
    async fn verify_token(&self) -> Result<UserInfo, ProviderError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                format!("{}/api/user", self.base_url),
                None,
            )
            .send()
            .await?;
        let user: OgUser = check_response(resp).await?.json().await?;
        Ok(UserInfo {
            login: user.login,
            name: user.username,
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
            let mut url = format!("{}/api/gists?per_page=100&page={}", self.base_url, page);
            if let Some(since) = since {
                // Z 后缀 RFC3339，避免 +00:00 被百分号编码
                url.push_str(&format!(
                    "&since={}",
                    since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                ));
            }
            let resp = self
                .request(reqwest::Method::GET, url, None)
                .send()
                .await?;
            let gists: Vec<OgGist> = check_response(resp).await?.json().await?;
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
                format!("{}/api/gists/{gist_id}", self.base_url),
                None,
            )
            .send()
            .await?;
        let g = check_response(resp).await?.json::<OgGist>().await?;
        let (mut detail, sha) = to_detail(g);
        self.backfill_raw_content(&mut detail, sha.as_deref(), wanted_file)
            .await?;
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
                format!("{}/api/gists/{gist_id}", self.base_url),
                Some(body),
            )
            .send()
            .await?;
        let g = check_response(resp).await?.json::<OgGist>().await?;
        let (mut detail, sha) = to_detail(g);
        // PATCH 响应只关心被改的文件（回读校验用）
        self.backfill_raw_content(&mut detail, sha.as_deref(), Some(filename))
            .await?;
        Ok(detail)
    }

    async fn list_commits(&self, gist_id: &str) -> Result<Vec<GistCommit>, ProviderError> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut page: u32 = 1;
        loop {
            let url = format!(
                "{}/api/gists/{gist_id}/commits?per_page=100&page={page}",
                self.base_url
            );
            let resp = self
                .request(reqwest::Method::GET, url, None)
                .send()
                .await?;
            let commits: Vec<OgCommitEntry> = check_response(resp).await?.json().await?;
            let is_full_page = commits.len() == 100;
            for c in commits {
                if seen.insert(c.version.clone()) {
                    out.push(GistCommit {
                        revision: c.version,
                        committed_at: c.committed_at,
                    });
                }
            }
            if !is_full_page {
                break;
            }
            if page >= super::MAX_LIST_PAGES {
                tracing::warn!(
                    "list_commits 分页达安全上限 {} 页，提前停止（服务端疑似异常）",
                    super::MAX_LIST_PAGES
                );
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    async fn get_file_at_revision(
        &self,
        gist_id: &str,
        rev: &str,
        filename: &str,
    ) -> Result<String, ProviderError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                format!("{}/api/gists/{gist_id}/{rev}", self.base_url),
                None,
            )
            .send()
            .await?;
        let g = check_response(resp).await?.json::<OgGist>().await?;
        let file = g
            .files
            .into_iter()
            .find(|(key, f)| f.filename.as_deref() == Some(filename) || key == filename)
            .ok_or(ProviderError::NotFound)?
            .1;
        // content 缺失或被截断 → 回退 raw 端点（带指定版本）
        if let Some(c) = file.content.filter(|_| !file.truncated) {
            return Ok(c);
        }
        let raw_url = format!(
            "{}/api/gists/{gist_id}/files/{rev}/{filename}",
            self.base_url
        );
        let resp = self
            .request(reqwest::Method::GET, raw_url, None)
            .send()
            .await?;
        Ok(check_response(resp).await?.text().await?)
    }
}

impl OpengistProvider {
    /// 文件 content 缺失或被截断时，经 raw 端点回补（sha 用详情响应 commits[0].version）。
    /// `wanted_file`：只回补该文件，避免为 gist 内无关文件放大流量；None 全部回补。
    async fn backfill_raw_content(
        &self,
        detail: &mut GistDetail,
        sha: Option<&str>,
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
            let Some(sha) = sha else { continue };
            let url = format!(
                "{}/api/gists/{}/files/{}/{}",
                self.base_url,
                detail.id,
                sha,
                f.filename
            );
            let resp = self
                .request(reqwest::Method::GET, url, None)
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

    fn provider(server: &MockServer) -> OpengistProvider {
        OpengistProvider::new(server.uri(), "og_test_token".into())
    }

    fn gist_json(
        id: &str,
        description: &str,
        filename: &str,
        content: Option<&str>,
        truncated: bool,
        with_commit: bool,
    ) -> serde_json::Value {
        let file = json!({
            "filename": filename,
            "type": "text/plain",
            "language": "Markdown",
            "size": 5,
            "truncated": truncated,
            "content": content,
            "encoding": "utf-8"
        });
        let mut g = json!({
            "id": id,
            "description": description,
            "updated_at": "2026-10-13T08:00:00Z",
            "title": "t",
            "files": { filename: file }
        });
        if with_commit {
            g["commits"] = json!([{ "version": "deadbeef01" }]);
        }
        g
    }

    #[tokio::test]
    async fn list_gists_stops_at_max_pages_on_runaway_server() {
        // 异常服务端恒返满页：分页循环必须在安全上限处停止（防死循环）
        let server = MockServer::start().await;
        let full_page: Vec<_> = (0..100)
            .map(|i| gist_json(&format!("g{i}"), "d", "a.txt", Some("x"), false, true))
            .collect();
        Mock::given(method("GET"))
            .and(path("/api/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(full_page)))
            .expect(crate::provider::MAX_LIST_PAGES as u64)
            .mount(&server)
            .await;
        let out = provider(&server).list_gists(None).await.unwrap();
        assert_eq!(out.len(), 100, "同页内容去重后应恰为 100 条");
        server.verify().await; // 请求数恰好停在安全上限
    }

    #[tokio::test]
    async fn list_gists_parses_title_when_present_and_absent() {
        let server = MockServer::start().await;
        let mk = |id: &str, title: Option<&str>| {
            let mut g = json!({
                "id": id,
                "description": "描述",
                "updated_at": "2026-10-13T08:00:00Z",
            });
            if let Some(t) = title {
                g["title"] = json!(t);
            }
            g
        };
        Mock::given(method("GET"))
            .and(path("/api/gists"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                mk("g1", Some("配置笔记")),
                mk("g2", None),
            ])))
            .mount(&server)
            .await;
        let gists = provider(&server).list_gists(None).await.unwrap();
        assert_eq!(gists[0].title.as_deref(), Some("配置笔记"));
        assert_eq!(gists[1].title, None, "响应无 title 键时容忍为 None");
    }

    #[tokio::test]
    async fn verify_token_uses_bearer_and_maps_username() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/user"))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer og_test_token",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 1, "login": "alice", "username": "Alice", "email": ""
            })))
            .expect(1)
            .mount(&server)
            .await;
        let user = provider(&server).verify_token().await.unwrap();
        assert_eq!(user.login, "alice");
        assert_eq!(user.name.as_deref(), Some("Alice"));
    }

    #[tokio::test]
    async fn list_gists_paginates_and_forwards_since() {
        let server = MockServer::start().await;
        // GistSimple 响应不含 files 字段，构造时剔除
        let strip_files = |mut g: serde_json::Value| {
            g.as_object_mut().unwrap().remove("files");
            g
        };
        let page1: Vec<_> = (0..100)
            .map(|i| strip_files(gist_json(&format!("g{i}"), "d", "a.txt", Some("x"), false, false)))
            .collect();
        Mock::given(method("GET"))
            .and(path("/api/gists"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .and(query_param("since", "2026-10-01T00:00:00Z"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(page1)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/gists"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                strip_files(gist_json("g-last", "最终", "b.md", Some("y"), false, false))
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let gists = provider(&server)
            .list_gists(Some(chrono::Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()))
            .await
            .unwrap();
        assert_eq!(gists.len(), 101);
        assert_eq!(gists[100].id, "g-last");
        // GistSimple 无 files 字段 → 空
        assert!(gists[0].files.is_empty());
    }

    #[tokio::test]
    async fn get_gist_parses_files_with_content() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json(
                "uuid1",
                "笔记",
                "notes.md",
                Some("hello"),
                false,
                true,
            )))
            .expect(1)
            .mount(&server)
            .await;
        let detail = provider(&server).get_gist("uuid1", Some("notes.md")).await.unwrap();
        assert_eq!(detail.files[0].filename, "notes.md");
        assert_eq!(detail.files[0].content.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn get_gist_truncated_content_backfills_via_raw() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json(
                "uuid1",
                "大文件",
                "big.md",
                None,
                true,
                true,
            )))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/files/deadbeef01/big.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("full content from raw"))
            .expect(1)
            .mount(&server)
            .await;
        let detail = provider(&server).get_gist("uuid1", Some("big.md")).await.unwrap();
        assert_eq!(
            detail.files[0].content.as_deref(),
            Some("full content from raw")
        );
    }

    #[tokio::test]
    async fn get_gist_backfills_only_wanted_file() {
        // 双文件 gist：只有目标文件回补 raw，无关文件即使缺 content 也不拉（流量不放大）
        let server = MockServer::start().await;
        let body = json!({
            "id": "uuid1",
            "description": "双文件",
            "updated_at": "2026-10-13T08:00:00Z",
            "title": "t",
            "files": {
                "notes.md": { "filename": "notes.md", "truncated": true, "content": null },
                "other.md": { "filename": "other.md", "truncated": true, "content": null }
            },
            "commits": [{ "version": "deadbeef01" }]
        });
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/files/deadbeef01/notes.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("full content from raw"))
            .expect(1)
            .mount(&server)
            .await;

        let detail = provider(&server).get_gist("uuid1", Some("notes.md")).await.unwrap();
        // files 来自 HashMap，顺序不定：按文件名断言
        let wanted = detail.files.iter().find(|f| f.filename == "notes.md").unwrap();
        assert_eq!(wanted.content.as_deref(), Some("full content from raw"));
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
            .and(path("/api/gists/uuid1"))
            .and(body_json(expected_body))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json(
                "uuid1",
                "笔记",
                "notes.md",
                Some("new content"),
                false,
                true,
            )))
            .expect(1)
            .mount(&server)
            .await;
        let detail = provider(&server)
            .update_file("uuid1", "notes.md", "new content")
            .await
            .unwrap();
        assert_eq!(detail.files[0].content.as_deref(), Some("new content"));
    }

    #[tokio::test]
    async fn error_401_maps_to_auth() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/user"))
            .respond_with(ResponseTemplate::new(401).set_body_string("invalid token"))
            .mount(&server)
            .await;
        let err = provider(&server).verify_token().await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth), "got: {err:?}");
    }

    #[tokio::test]
    async fn error_403_api_disabled_body_maps_to_api_disabled() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/user"))
            .respond_with(ResponseTemplate::new(403).set_body_string(
                "{\"message\":\"the api is disabled on this instance\"}",
            ))
            .mount(&server)
            .await;
        let err = provider(&server).verify_token().await.unwrap_err();
        assert!(matches!(err, ProviderError::ApiDisabled), "got: {err:?}");
    }

    // ── 历史能力：commits 与指定版本内容 ──

    fn commit_json(version: &str, at: &str) -> serde_json::Value {
        json!({
            "version": version,
            "committed_at": at,
            "author": { "email": "a@b.c", "name": "a" },
            "change_status": { "additions": 1, "deletions": 0, "total": 1 }
        })
    }

    #[tokio::test]
    async fn list_commits_maps_version_and_committed_at() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/commits"))
            .and(query_param("per_page", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                commit_json("deadbeef01", "2026-10-13T08:00:00Z"),
                commit_json("deadbeef02", "2026-10-12T08:00:00Z")
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let commits = provider(&server).list_commits("uuid1").await.unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].revision, "deadbeef01");
        assert_eq!(
            commits[0].committed_at,
            chrono::Utc.with_ymd_and_hms(2026, 10, 13, 8, 0, 0).unwrap()
        );
    }

    #[tokio::test]
    async fn list_commits_paginates_until_short_page() {
        let server = MockServer::start().await;
        let page1: Vec<_> = (0..100)
            .map(|i| commit_json(&format!("dead{i:07}"), "2026-10-13T08:00:00Z"))
            .collect();
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/commits"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(page1)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/commits"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                commit_json("dead-last", "2026-10-01T08:00:00Z")
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let commits = provider(&server).list_commits("uuid1").await.unwrap();
        assert_eq!(commits.len(), 101, "两页后不满页即停止");
        server.verify().await;
    }

    #[tokio::test]
    async fn list_commits_stops_at_max_pages_on_runaway_server() {
        // 异常服务端恒返满页：分页循环必须在安全上限处停止（防死循环）
        let server = MockServer::start().await;
        let full_page: Vec<_> = (0..100)
            .map(|i| commit_json(&format!("dead{i:07}"), "2026-10-13T08:00:00Z"))
            .collect();
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/commits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(full_page)))
            .expect(crate::provider::MAX_LIST_PAGES as u64)
            .mount(&server)
            .await;
        let commits = provider(&server).list_commits("uuid1").await.unwrap();
        assert_eq!(commits.len(), 100, "同页内容去重后应恰为 100 条");
        server.verify().await;
    }

    #[tokio::test]
    async fn get_file_at_revision_returns_embedded_content() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/deadbeef01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json(
                "uuid1",
                "版本1",
                "notes.md",
                Some("v1 content"),
                false,
                true,
            )))
            .expect(1)
            .mount(&server)
            .await;
        let content = provider(&server)
            .get_file_at_revision("uuid1", "deadbeef01", "notes.md")
            .await
            .unwrap();
        assert_eq!(content, "v1 content");
        server.verify().await; // content 直接可用，不应再发 raw 请求
    }

    #[tokio::test]
    async fn get_file_at_revision_truncated_content_falls_back_to_raw() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/deadbeef01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gist_json(
                "uuid1",
                "大文件",
                "big.md",
                None,
                true,
                true,
            )))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/files/deadbeef01/big.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("full raw content"))
            .expect(1)
            .mount(&server)
            .await;
        let content = provider(&server)
            .get_file_at_revision("uuid1", "deadbeef01", "big.md")
            .await
            .unwrap();
        assert_eq!(content, "full raw content");
    }

    #[tokio::test]
    async fn get_file_at_revision_unknown_revision_maps_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/gists/uuid1/nosuchrev"))
            .respond_with(ResponseTemplate::new(404).set_body_string("gist not found"))
            .mount(&server)
            .await;
        let err = provider(&server)
            .get_file_at_revision("uuid1", "nosuchrev", "notes.md")
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::NotFound), "got: {err:?}");
    }
}
