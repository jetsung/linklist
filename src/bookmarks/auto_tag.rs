use crate::db::DbPool;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// 单条自动打标规则。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoTagRule {
    /// 规则名（可选，便于管理）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 匹配字段：url / title / any。
    pub field: String,
    /// 匹配方式：contains（大小写不敏感）。
    #[serde(default = "default_match")]
    pub match_type: String,
    /// 匹配关键词。
    pub pattern: String,
    /// 命中时附加的标签。
    pub tags: Vec<String>,
}

fn default_match() -> String {
    "contains".into()
}

/// 读取用户规则（JSON 存于 user_profiles.auto_tagging_rules）。
pub async fn get_rules(pool: &DbPool, user_id: i64) -> Result<Vec<AutoTagRule>, AppError> {
    ensure_profile(pool, user_id).await?;
    let raw: String =
        crate::db::q_scalar::<()>("SELECT auto_tagging_rules FROM user_profiles WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(pool)
            .await?
            .get_opt_string(0)
            .unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&raw).map_err(|e| AppError::internal(format!("规则解析失败: {e}")))
}

/// 保存用户规则。
pub async fn set_rules(pool: &DbPool, user_id: i64, rules: &[AutoTagRule]) -> Result<(), AppError> {
    ensure_profile(pool, user_id).await?;
    let raw = serde_json::to_string(rules).map_err(AppError::internal)?;
    crate::db::q("UPDATE user_profiles SET auto_tagging_rules = ? WHERE user_id = ?")
        .bind(&raw)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 确保用户 profile 行存在。
async fn ensure_profile(pool: &DbPool, user_id: i64) -> Result<(), AppError> {
    crate::db::q(crate::db::sql_insert_ignore(
        pool.dialect(),
        "user_profiles (user_id)",
        "?",
    ))
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 对 URL + 标题执行规则匹配，返回应附加的标签（去重保序）。
pub fn match_tags(rules: &[AutoTagRule], url: &str, title: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let url_lower = url.to_ascii_lowercase();
    let title_lower = title.to_ascii_lowercase();
    for rule in rules {
        if rule.pattern.is_empty() {
            continue;
        }
        let hit = match rule.field.as_str() {
            "url" => url_lower.contains(&rule.pattern.to_ascii_lowercase()),
            "title" => title_lower.contains(&rule.pattern.to_ascii_lowercase()),
            _ => {
                url_lower.contains(&rule.pattern.to_ascii_lowercase())
                    || title_lower.contains(&rule.pattern.to_ascii_lowercase())
            }
        };
        if hit {
            for t in &rule.tags {
                if !t.trim().is_empty() && !tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                    tags.push(t.trim().to_string());
                }
            }
        }
    }
    tags
}

/// 创建/导入时应用：把命中标签并入 existing。
pub fn apply_rules(rules: &[AutoTagRule], url: &str, title: &str, existing: &mut Vec<String>) {
    for t in match_tags(rules, url, title) {
        if !existing.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
            existing.push(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(field: &str, pattern: &str, tags: &[&str]) -> AutoTagRule {
        AutoTagRule {
            name: String::new(),
            field: field.into(),
            match_type: "contains".into(),
            pattern: pattern.into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn url_match() {
        let rules = vec![rule("url", "github.com", &["git", "code"])];
        let tags = match_tags(&rules, "https://github.com/foo/bar", "FB");
        assert_eq!(tags, vec!["git", "code"]);
    }

    #[test]
    fn title_match_case_insensitive() {
        let rules = vec![rule("title", "RUST", &["rust"])];
        let tags = match_tags(&rules, "https://a.com", "Learn Rust Today");
        assert_eq!(tags, vec!["rust"]);
    }

    #[test]
    fn any_match() {
        let rules = vec![rule("any", "news", &["news"])];
        assert!(match_tags(&rules, "https://a.com", "t").is_empty());
        assert_eq!(match_tags(&rules, "https://news.com", "t"), vec!["news"]);
        assert_eq!(
            match_tags(&rules, "https://a.com", "Daily News"),
            vec!["news"]
        );
    }

    #[test]
    fn apply_merges_without_dup() {
        let rules = vec![rule("url", "github", &["git"])];
        let mut existing = vec!["own".into()];
        apply_rules(&rules, "https://github.com/x", "", &mut existing);
        assert_eq!(existing, vec!["own", "git"]);
        // 已有标签不重复。
        let mut existing2 = vec!["git".into()];
        apply_rules(&rules, "https://github.com/x", "", &mut existing2);
        assert_eq!(existing2, vec!["git"]);
    }

    #[test]
    fn empty_pattern_never_matches() {
        let rules = vec![rule("url", "", &["x"])];
        assert!(match_tags(&rules, "https://a.com", "").is_empty());
    }

    #[tokio::test]
    async fn rules_persist_roundtrip() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        assert!(get_rules(&pool, u.id).await.unwrap().is_empty());
        let rules = vec![rule("url", "github.com", &["git"])];
        set_rules(&pool, u.id, &rules).await.unwrap();
        let loaded = get_rules(&pool, u.id).await.unwrap();
        assert_eq!(loaded, rules);
    }
}
