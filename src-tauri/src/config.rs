use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use thiserror::Error;

use crate::model::AppConfig;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("配置文件损坏: {0}")]
    Corrupted(String),
}

/// config.json 的原子读写存储。
/// 先写磁盘再提交内存，保证磁盘成功后内存才更新。
#[derive(Debug)]
pub struct ConfigStore {
    path: PathBuf,
    config: RwLock<AppConfig>,
}

impl ConfigStore {
    /// 从指定路径加载；文件不存在 → 返回默认配置（不视为错误）；JSON 损坏 → Corrupted。
    pub fn load_from(path: PathBuf) -> Result<Self, ConfigError> {
        let config = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| ConfigError::Corrupted(e.to_string()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AppConfig::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            path,
            config: RwLock::new(config),
        })
    }

    /// 读取当前配置的快照
    pub fn read(&self) -> AppConfig {
        self.config.read().expect("config lock poisoned").clone()
    }

    /// 应用变更并原子落盘；返回更新后的配置快照
    pub fn update<F: FnOnce(&mut AppConfig)>(&self, f: F) -> Result<AppConfig, ConfigError> {
        self.try_update::<_, std::convert::Infallible>(|c| {
            f(c);
            Ok(())
        })
        .map(|r| r.expect("Infallible 不可能为 Err"))
    }

    /// 校验+变更原子化：闭包返回 Err 时不落盘不改内存（消除唯一性校验 TOCTOU）
    pub fn try_update<F, E>(&self, f: F) -> Result<Result<AppConfig, E>, ConfigError>
    where
        F: FnOnce(&mut AppConfig) -> Result<(), E>,
    {
        let mut next = self.read();
        if let Err(e) = f(&mut next) {
            return Ok(Err(e));
        }
        Self::save_to_disk(&next, &self.path)?;
        *self.config.write().expect("config lock poisoned") = next.clone();
        Ok(Ok(next))
    }

    pub fn config_path(&self) -> &Path {
        &self.path
    }

    fn save_to_disk(config: &AppConfig, path: &Path) -> Result<(), ConfigError> {
        let json = serde_json::to_string_pretty(config)
            .map_err(|e| ConfigError::Corrupted(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Account, Mapping, ProviderKind};

    fn sample_account() -> Account {
        Account {
            id: "acc-1".into(),
            name: "我的 GitHub".into(),
            provider: ProviderKind::Github,
            base_url: "https://api.github.com".into(),
            token_ref: "gistsync:account:acc-1".into(),
        }
    }

    fn sample_mapping() -> Mapping {
        Mapping {
            id: "map-1".into(),
            local_path: "C:/notes/todo.md".into(),
            account_id: "acc-1".into(),
            gist_id: "abcdef123456".into(),
            gist_filename: "todo.md".into(),
            upload_enabled: true,
            download_enabled: true,
            enabled: true,
        }
    }

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent-config.json");
        let store = ConfigStore::load_from(path).unwrap();
        let cfg = store.read();
        assert_eq!(cfg.settings.debounce_seconds, 3);
        assert!(cfg.accounts.is_empty());
    }

    #[test]
    fn legacy_settings_without_auto_check_defaults_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        // V0.4 之前的配置文件没有 autoCheckUpdate 键，加载必须成功且默认开启
        let legacy = serde_json::json!({
            "settings": {
                "debounceSeconds": 3,
                "pollIntervalMinutes": 5,
                "autoStart": true,
                "logLevel": "info",
                "language": "zh-CN"
            },
            "accounts": [],
            "mappings": []
        });
        fs::write(&path, legacy.to_string()).unwrap();
        let store = ConfigStore::load_from(path).unwrap();
        assert!(store.read().settings.auto_check_update);
    }

    #[test]
    fn default_settings_auto_check_on() {
        assert!(crate::model::GlobalSettings::default().auto_check_update);
    }

    #[test]
    fn corrupted_json_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{ not valid json !!").unwrap();
        let err = ConfigStore::load_from(path).unwrap_err();
        assert!(matches!(err, ConfigError::Corrupted(_)), "got: {err:?}");
    }

    #[test]
    fn update_saves_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let store = ConfigStore::load_from(path.clone()).unwrap();

        store
            .update(|c| {
                c.accounts.push(sample_account());
                c.mappings.push(sample_mapping());
                c.settings.debounce_seconds = 10;
            })
            .unwrap();

        let reloaded = ConfigStore::load_from(path).unwrap();
        let cfg = reloaded.read();
        assert_eq!(cfg.accounts, vec![sample_account()]);
        assert_eq!(cfg.mappings, vec![sample_mapping()]);
        assert_eq!(cfg.settings.debounce_seconds, 10);
    }

    #[test]
    fn try_update_rejects_without_side_effect() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let store = ConfigStore::load_from(path.clone()).unwrap();

        let err = "重复";
        let outcome: Result<_, String> = store
            .try_update::<_, String>(|_| Err(err.to_string()))
            .unwrap()
            .map(|_| ())
        ;
        assert_eq!(outcome.unwrap_err(), err);
        // 未落盘：文件不存在；内存未变
        assert!(!path.exists());
        assert!(store.read().accounts.is_empty());

        // Ok 路径正常落盘
        store
            .try_update::<_, String>(|c| {
                c.settings.debounce_seconds = 7;
                Ok(())
            })
            .unwrap()
            .unwrap();
        assert_eq!(store.read().settings.debounce_seconds, 7);
    }

    #[test]
    fn config_file_contains_no_token_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let store = ConfigStore::load_from(path.clone()).unwrap();
        store.update(|c| c.accounts.push(sample_account())).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("ghp_"));
        assert!(!raw.contains("og_"));
        assert!(raw.contains("tokenRef"));
    }
}
