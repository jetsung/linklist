use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use base64::Engine;

use crate::error::AppError;

/// 生成 argon2id 口令哈希（PHC 字符串格式，含随机盐）。
pub fn hash_password(password: &str) -> Result<String, AppError> {
    // 16 字节随机盐（rand 0.10 生成；password-hash 0.5 的 OsRng 路径依赖已断开）。
    let mut salt_bytes = [0u8; 16];
    rand::fill(&mut salt_bytes);
    let salt = SaltString::from_b64(
        base64::engine::general_purpose::STANDARD_NO_PAD
            .encode(salt_bytes)
            .as_str(),
    )
    .map_err(|e| AppError::internal(format!("盐生成失败: {e}")))?;
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AppError::internal(format!("口令哈希失败: {e}")))?;
    Ok(hash.to_string())
}

/// 验证口令与 PHC 哈希是否匹配。
pub fn verify_password(password: &str, hash: &str) -> Result<bool, AppError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| AppError::internal(format!("口令哈希解析失败: {e}")))?;
    let result = Argon2::default().verify_password(password.as_bytes(), &parsed);
    Ok(result.is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let hash = hash_password("s3cret-密码!").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("s3cret-密码!", &hash).unwrap());
    }

    #[test]
    fn wrong_password_fails() {
        let hash = hash_password("correct").unwrap();
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[test]
    fn salt_makes_hashes_unique() {
        let a = hash_password("same").unwrap();
        let b = hash_password("same").unwrap();
        assert_ne!(a, b, "相同口令应产生不同盐与哈希");
    }

    #[test]
    fn invalid_hash_is_internal_error() {
        let result = verify_password("x", "not-a-phc-hash");
        assert!(result.is_err());
    }
}
