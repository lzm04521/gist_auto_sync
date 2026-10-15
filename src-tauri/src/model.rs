use serde::{Deserialize, Serialize};

/// gist 服务提供方类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Github,
    Opengist,
}

/// 一个 gist 服务账号；token 本体只存 Windows 凭据管理器，这里只存引用
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    pub provider: ProviderKind,
    /// github 固定 https://api.github.com；opengist 为实例根地址
    pub base_url: String,
    /// keyring 条目引用：gistsync:account:<id>
    pub token_ref: String,
}

/// 一条同步映射：本地文件 ↔ 某账号某 gist 内的某个文件
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    pub id: String,
    pub local_path: String,
    pub account_id: String,
    pub gist_id: String,
    pub gist_filename: String,
    pub upload_enabled: bool,
    pub download_enabled: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSettings {
    pub debounce_seconds: u64,
    pub poll_interval_minutes: u64,
    pub auto_start: bool,
    pub log_level: String,
    pub language: String,
    /// 启动后是否自动检查应用更新（V0.4 新增，旧配置缺键时默认开启）
    #[serde(default = "default_auto_check_update")]
    pub auto_check_update: bool,
}

fn default_auto_check_update() -> bool {
    true
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            debounce_seconds: 3,
            poll_interval_minutes: 5,
            auto_start: true,
            log_level: "info".to_string(),
            language: "zh-CN".to_string(),
            auto_check_update: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub mappings: Vec<Mapping>,
    #[serde(default)]
    pub settings: GlobalSettings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_settings_defaults_match_spec() {
        let s = GlobalSettings::default();
        assert_eq!(s.debounce_seconds, 3);
        assert_eq!(s.poll_interval_minutes, 5);
        assert!(s.auto_start);
        assert_eq!(s.log_level, "info");
        assert_eq!(s.language, "zh-CN");
    }

    #[test]
    fn account_serializes_camel_case_without_token_field() {
        let account = Account {
            id: "a1".into(),
            name: "个人 GitHub".into(),
            provider: ProviderKind::Github,
            base_url: "https://api.github.com".into(),
            token_ref: "gistsync:account:a1".into(),
        };
        let value = serde_json::to_value(&account).unwrap();
        let obj = value.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["baseUrl", "id", "name", "provider", "tokenRef"]);
    }

    #[test]
    fn provider_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(ProviderKind::Github).unwrap(),
            serde_json::json!("github")
        );
        assert_eq!(
            serde_json::to_value(ProviderKind::Opengist).unwrap(),
            serde_json::json!("opengist")
        );
    }

    #[test]
    fn empty_config_deserializes_with_defaults() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.accounts.is_empty());
        assert!(cfg.mappings.is_empty());
        assert_eq!(cfg.settings.debounce_seconds, 3);
    }
}
