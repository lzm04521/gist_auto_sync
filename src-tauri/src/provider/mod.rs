use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

use crate::model::{Account, ProviderKind};

pub mod github;
pub mod opengist;

/// 认证用户信息（verify_token 返回）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub login: String,
    pub name: Option<String>,
}

/// gist 列表项（下拉选择用）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GistSummary {
    pub id: String,
    pub description: Option<String>,
    pub updated_at: DateTime<Utc>,
    /// gist 内全部文件名
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GistFile {
    pub filename: String,
    pub raw_url: Option<String>,
    /// 详情响应可能不含/截断 content；缺失时由 provider 回退拉取 raw_url
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GistDetail {
    pub id: String,
    pub description: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub files: Vec<GistFile>,
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum ProviderError {
    #[error("网络错误: {0}")]
    Network(String),
    #[error("认证失败：token 无效或已过期")]
    Auth,
    #[error("无权限访问该资源")]
    Forbidden,
    #[error("触发服务限流（retry_after={retry_after:?}）")]
    RateLimited {
        retry_after: Option<Duration>,
    },
    #[error("资源不存在")]
    NotFound,
    #[error("服务端 API 已禁用（Opengist api.enabled=false）")]
    ApiDisabled,
    #[error("未知错误（HTTP {0}）: {1}")]
    Unknown(u16, String),
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        ProviderError::Network(e.to_string())
    }
}

/// 把 HTTP 状态映射为错误分类。
/// `retry_after`：429 时响应的 Retry-After 头（秒）。
/// `body_hint`：响应体片段，用于识别 Opengist "API disabled" 的 403。
pub fn map_status(status: reqwest::StatusCode, retry_after: Option<&str>, body_hint: &str) -> ProviderError {
    match status.as_u16() {
        401 => ProviderError::Auth,
        403 => {
            let hint = body_hint.to_ascii_lowercase();
            if hint.contains("api") && hint.contains("disabl") {
                ProviderError::ApiDisabled
            } else {
                ProviderError::Forbidden
            }
        }
        404 => ProviderError::NotFound,
        429 => ProviderError::RateLimited {
            retry_after: parse_retry_after(retry_after),
        },
        _ => ProviderError::Unknown(status.as_u16(), body_hint.to_string()),
    }
}

fn parse_retry_after(raw: Option<&str>) -> Option<Duration> {
    raw?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// gist 服务统一接口：GitHub Gist 与 Opengist 各一实现。
/// 注意：原生 async fn in trait 在 rustc 1.95 仍不兼容 dyn（E0038），
/// Box<dyn GistProvider> 需要 async_trait 宏做 Box 化（见台账 Task 4 裁决）。
#[async_trait::async_trait]
pub trait GistProvider: Send + Sync {
    /// "测试连接"：校验 token 有效性并返回认证用户
    async fn verify_token(&self) -> Result<UserInfo, ProviderError>;
    /// 列当前用户 gists；since 用于轮询增量过滤
    async fn list_gists(&self, since: Option<DateTime<Utc>>) -> Result<Vec<GistSummary>, ProviderError>;
    /// 取 gist 详情（含全部文件；目标文件 content 缺失时由实现回退拉 raw）
    async fn get_gist(&self, gist_id: &str) -> Result<GistDetail, ProviderError>;
    /// 更新 gist 内单个文件（PATCH 只带该文件一个 key）；文件不存在则新增
    async fn update_file(&self, gist_id: &str, filename: &str, content: &str) -> Result<GistDetail, ProviderError>;
}

/// 按账号类型构造 provider
pub fn build_provider(account: &Account, token: &str) -> Box<dyn GistProvider> {
    match account.provider {
        ProviderKind::Github => Box::new(crate::provider::github::GithubProvider::new(
            token.to_string(),
        )),
        ProviderKind::Opengist => Box::new(crate::provider::opengist::OpengistProvider::new(
            account.base_url.clone(),
            token.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn map_status_matrix() {
        assert!(matches!(
            map_status(StatusCode::UNAUTHORIZED, None, ""),
            ProviderError::Auth
        ));
        assert!(matches!(
            map_status(StatusCode::FORBIDDEN, None, "this instance has api disabled"),
            ProviderError::ApiDisabled
        ));
        assert!(matches!(
            map_status(StatusCode::FORBIDDEN, None, "resource not accessible"),
            ProviderError::Forbidden
        ));
        assert!(matches!(
            map_status(StatusCode::NOT_FOUND, None, ""),
            ProviderError::NotFound
        ));
        match map_status(StatusCode::TOO_MANY_REQUESTS, Some("120"), "") {
            ProviderError::RateLimited { retry_after: Some(d) } => assert_eq!(d, Duration::from_secs(120)),
            other => panic!("expected RateLimited(120s), got {other:?}"),
        }
        match map_status(StatusCode::TOO_MANY_REQUESTS, Some("not-a-number"), "") {
            ProviderError::RateLimited { retry_after: None } => {}
            other => panic!("expected RateLimited(None), got {other:?}"),
        }
        match map_status(StatusCode::INTERNAL_SERVER_ERROR, None, "boom") {
            ProviderError::Unknown(500, msg) => assert!(msg.contains("boom")),
            other => panic!("expected Unknown(500), got {other:?}"),
        }
    }
}
