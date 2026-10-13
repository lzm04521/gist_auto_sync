use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretsError {
    #[error("平台密钥库错误: {0}")]
    Keyring(String),
    #[error("凭据不存在: {0}")]
    NotFound(String),
}

/// keyring 条目的 service 名
pub const KEYRING_SERVICE: &str = "gistsync";

/// 账号 id → keyring 条目引用（account 名）
pub fn token_ref_for(account_id: &str) -> String {
    format!("{KEYRING_SERVICE}:account:{account_id}")
}

pub fn save_token(token_ref: &str, token: &str) -> Result<(), SecretsError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, token_ref).map_err(keyring_err)?;
    entry.set_password(token).map_err(keyring_err)
}

pub fn load_token(token_ref: &str) -> Result<String, SecretsError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, token_ref).map_err(keyring_err)?;
    match entry.get_password() {
        Ok(token) => Ok(token),
        Err(keyring::Error::NoEntry) => Err(SecretsError::NotFound(token_ref.to_string())),
        Err(e) => Err(keyring_err(e)),
    }
}

pub fn delete_token(token_ref: &str) -> Result<(), SecretsError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, token_ref).map_err(keyring_err)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(keyring_err(e)),
    }
}

fn keyring_err(e: keyring::Error) -> SecretsError {
    SecretsError::Keyring(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试用独立 uuid 引用，避免串扰真实凭据
    fn fresh_ref() -> String {
        format!("test:{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn token_ref_format() {
        assert_eq!(token_ref_for("acc-1"), "gistsync:account:acc-1");
    }

    #[test]
    fn save_load_delete_roundtrip() {
        let r = fresh_ref();
        save_token(&r, "ghp_unit_test_token").unwrap();
        assert_eq!(load_token(&r).unwrap(), "ghp_unit_test_token");
        delete_token(&r).unwrap();
        assert!(matches!(load_token(&r), Err(SecretsError::NotFound(_))));
    }

    #[test]
    fn delete_missing_is_ok() {
        let r = fresh_ref();
        delete_token(&r).unwrap();
    }
}
