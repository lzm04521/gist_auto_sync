/// Opengist REST API 实现（Task 6 填充）
// 字段在 Task 6 实现后开始使用
#[allow(dead_code)]
pub struct OpengistProvider {
    // 字段在 Task 6 实现后开始使用
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
}

#[async_trait::async_trait]
impl crate::provider::GistProvider for OpengistProvider {
    async fn verify_token(&self) -> Result<crate::provider::UserInfo, crate::provider::ProviderError> {
        todo!()
    }
    async fn list_gists(
        &self,
        _since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<crate::provider::GistSummary>, crate::provider::ProviderError> {
        todo!()
    }
    async fn get_gist(
        &self,
        _gist_id: &str,
    ) -> Result<crate::provider::GistDetail, crate::provider::ProviderError> {
        todo!()
    }
    async fn update_file(
        &self,
        _gist_id: &str,
        _filename: &str,
        _content: &str,
    ) -> Result<crate::provider::GistDetail, crate::provider::ProviderError> {
        todo!()
    }
}
