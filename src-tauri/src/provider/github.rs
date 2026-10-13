/// GitHub Gist REST API 实现（Task 5 填充）
// 字段在 Task 5 实现后开始使用
#[allow(dead_code)]
pub struct GithubProvider {
    // 字段在 Task 5 实现后开始使用
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
            base_url,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl crate::provider::GistProvider for GithubProvider {
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
