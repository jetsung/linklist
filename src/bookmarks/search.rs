use crate::error::AppError;

/// 搜索表达式 AST。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// 纯关键词（对 FTS 列匹配）。
    Keyword(String),
    /// 短语（引号包裹）。
    Phrase(String),
    /// 标签筛选：tag:name。
    Tag(String),
    /// 未读筛选：unread。
    Unread,
    /// 无标签：untagged。
    Untagged,
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}

/// 词法单元。
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Phrase(String),
    LParen,
    RParen,
    And,
    Or,
    Not,
}

/// 词法分析：支持引号短语、括号、AND/OR/NOT（大小写不敏感）、tag: 前缀。
fn tokenize(input: &str) -> Result<Vec<Tok>, AppError> {
    let mut toks = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            _ if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                toks.push(Tok::LParen);
                chars.next();
            }
            ')' => {
                toks.push(Tok::RParen);
                chars.next();
            }
            '"' => {
                chars.next();
                let mut phrase = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '"' {
                        closed = true;
                        break;
                    }
                    phrase.push(ch);
                }
                if !closed {
                    return Err(AppError::BadRequest("引号未闭合".into()));
                }
                if phrase.trim().is_empty() {
                    return Err(AppError::BadRequest("空短语".into()));
                }
                toks.push(Tok::Phrase(phrase.trim().to_string()));
            }
            _ => {
                let mut word = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_whitespace() || ch == '(' || ch == ')' || ch == '"' {
                        break;
                    }
                    word.push(ch);
                    chars.next();
                }
                let lower = word.to_ascii_lowercase();
                match lower.as_str() {
                    "and" => toks.push(Tok::And),
                    "or" => toks.push(Tok::Or),
                    "not" => toks.push(Tok::Not),
                    _ => toks.push(Tok::Word(word)),
                }
            }
        }
    }
    Ok(toks)
}

/// 递归下降解析：
/// expr    := or_expr
/// or_expr := and_expr (OR and_expr)*
/// and_expr:= not_expr ((AND)? not_expr)*   （相邻项隐式 AND）
/// not_expr:= NOT not_expr | primary
/// primary := '(' expr ')' | phrase | tag:word | unread | untagged | word
pub fn parse_query(input: &str) -> Result<Expr, AppError> {
    let toks = tokenize(input)?;
    if toks.is_empty() {
        return Err(AppError::BadRequest("搜索表达式为空".into()));
    }
    let mut pos = 0usize;
    let expr = parse_or(&toks, &mut pos)?;
    if pos != toks.len() {
        return Err(AppError::BadRequest("意外的词元（如多余右括号）".into()));
    }
    Ok(expr)
}

fn parse_or(toks: &[Tok], pos: &mut usize) -> Result<Expr, AppError> {
    let mut left = parse_and(toks, pos)?;
    while *pos < toks.len() && toks[*pos] == Tok::Or {
        *pos += 1;
        let right = parse_and(toks, pos)?;
        left = Expr::Or(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_and(toks: &[Tok], pos: &mut usize) -> Result<Expr, AppError> {
    let mut left = parse_not(toks, pos)?;
    loop {
        match toks.get(*pos) {
            Some(Tok::And) => {
                *pos += 1;
                let right = parse_not(toks, pos)?;
                left = Expr::And(Box::new(left), Box::new(right));
            }
            // 隐式 AND：下一词元是可开始一个 primary 的词元。
            Some(Tok::Word(_)) | Some(Tok::Phrase(_)) | Some(Tok::LParen) | Some(Tok::Not) => {
                let right = parse_not(toks, pos)?;
                left = Expr::And(Box::new(left), Box::new(right));
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_not(toks: &[Tok], pos: &mut usize) -> Result<Expr, AppError> {
    if toks.get(*pos) == Some(&Tok::Not) {
        *pos += 1;
        let inner = parse_not(toks, pos)?;
        return Ok(Expr::Not(Box::new(inner)));
    }
    parse_primary(toks, pos)
}

fn parse_primary(toks: &[Tok], pos: &mut usize) -> Result<Expr, AppError> {
    match toks.get(*pos) {
        Some(Tok::LParen) => {
            *pos += 1;
            let inner = parse_or(toks, pos)?;
            if toks.get(*pos) != Some(&Tok::RParen) {
                return Err(AppError::BadRequest("缺少右括号".into()));
            }
            *pos += 1;
            Ok(inner)
        }
        Some(Tok::Phrase(p)) => {
            let e = Expr::Phrase(p.clone());
            *pos += 1;
            Ok(e)
        }
        Some(Tok::Word(w)) => {
            let e = if let Some(tag) = w.strip_prefix("tag:") {
                if tag.trim().is_empty() {
                    return Err(AppError::BadRequest("tag: 后缺少标签名".into()));
                }
                Expr::Tag(tag.trim().to_string())
            } else if let Some(tag) = w.strip_prefix('#') {
                // 对齐 linkding 的 #标签 搜索语法。
                if tag.trim().is_empty() {
                    return Err(AppError::BadRequest("# 后缺少标签名".into()));
                }
                Expr::Tag(tag.trim().to_string())
            } else if let Some(kw) = w.strip_prefix('!') {
                // 对齐 linkding 的 !特殊关键字 语法（!unread / !untagged）。
                match kw.to_ascii_lowercase().as_str() {
                    "unread" => Expr::Unread,
                    "untagged" => Expr::Untagged,
                    _ => Expr::Keyword(w.clone()),
                }
            } else if w.eq_ignore_ascii_case("unread") {
                Expr::Unread
            } else if w.eq_ignore_ascii_case("untagged") {
                Expr::Untagged
            } else {
                Expr::Keyword(w.clone())
            };
            *pos += 1;
            Ok(e)
        }
        Some(other) => Err(AppError::BadRequest(format!("意外的词元: {other:?}"))),
        None => Err(AppError::BadRequest("表达式意外结束".into())),
    }
}

/// 编译 AST 为 SQL WHERE 片段（参数追加到 binds）。
/// 返回的 SQL 引用 `b`（bookmarks 别名）。
///
/// 方言说明（三库通用，design D3）：
/// * icontains 用 `LOWER(col) LIKE ?`（绑定小写模式）：SQLite/MySQL/PG 均支持，
///   避免 SQLite 专有的 `COLLATE NOCASE` 与 PG 的 ILIKE 分叉。
/// * 布尔比较统一 `= TRUE` 字面量（SQLite 3.23+/MySQL/PG 均支持），PG 的 BOOLEAN
///   列不能与整数比较。
pub fn compile(expr: &Expr, binds: &mut Vec<String>) -> String {
    match expr {
        // 关键词/短语：对齐上游 Q(title|description|notes|url__icontains)——四列大小写
        // 不敏感的"包含"匹配（子串语义），而非 FTS5 整词匹配。FTS5 默认 unicode61
        // 分词下 "ping" 无法命中 "prototyping"/"pinggu.org"，与上游结果集不一致。
        Expr::Keyword(k) => {
            let term = like_pattern(k).to_lowercase();
            for _ in 0..4 {
                binds.push(term.clone());
            }
            "(LOWER(b.title) LIKE ? OR LOWER(b.description) LIKE ? \
              OR LOWER(b.notes) LIKE ? OR LOWER(b.url) LIKE ?)"
                .to_string()
        }
        // 短语：同一四列 icontains 语义（短语本身不再二次分词）。
        Expr::Phrase(p) => {
            let term = like_pattern(p).to_lowercase();
            for _ in 0..4 {
                binds.push(term.clone());
            }
            "(LOWER(b.title) LIKE ? OR LOWER(b.description) LIKE ? \
              OR LOWER(b.notes) LIKE ? OR LOWER(b.url) LIKE ?)"
                .to_string()
        }
        Expr::Tag(t) => {
            binds.push(t.to_ascii_lowercase());
            "b.id IN (SELECT bt.bookmark_id FROM bookmark_tags bt JOIN tags tg ON tg.id = bt.tag_id WHERE LOWER(tg.name) = ?)"
                .to_string()
        }
        Expr::Unread => "b.is_unread = TRUE".to_string(),
        Expr::Untagged => "b.id NOT IN (SELECT bookmark_id FROM bookmark_tags)".to_string(),
        Expr::And(l, r) => format!("({} AND {})", compile(l, binds), compile(r, binds)),
        Expr::Or(l, r) => format!("({} OR {})", compile(l, binds), compile(r, binds)),
        Expr::Not(inner) => format!("(NOT {})", compile(inner, binds)),
    }
}

/// LIKE 模式：转义通配符后做子串匹配（icontains 语义）。
fn like_pattern(term: &str) -> String {
    let escaped = term
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// 编译 AST 为 SQL WHERE 片段；lax 模式下普通词项额外兼查标签名（对齐上游 tag_search=lax）。
pub fn compile_lax(expr: &Expr, binds: &mut Vec<String>, lax_tags: bool) -> String {
    match expr {
        Expr::Keyword(k) if lax_tags => {
            // 关键词命中四列文本（icontains），或命中任一标签名（大小写不敏感）。
            let like = compile(&Expr::Keyword(k.clone()), binds);
            binds.push(k.to_ascii_lowercase());
            format!(
                "({like} OR b.id IN (SELECT bt.bookmark_id FROM bookmark_tags bt JOIN tags tg ON tg.id = bt.tag_id WHERE LOWER(tg.name) = ?))"
            )
        }
        _ => compile(expr, binds),
    }
}

/// 从 AST 提取查询中显式出现的标签名（tag:/#标签；lax 模式下普通词项也算标签）。
/// 对齐上游 extract_tag_names_from_query：大小写不敏感去重并排序。
#[allow(dead_code)] // TODO11 预留 API：标签云 selected 区后续消费
pub fn extract_tag_names(expr: &Expr, lax_tags: bool) -> Vec<String> {
    let mut names = Vec::new();
    collect_tag_names(expr, lax_tags, &mut names);
    let mut seen: Vec<String> = Vec::new();
    for n in names {
        let lower = n.to_ascii_lowercase();
        if !seen.iter().any(|s| s.eq_ignore_ascii_case(&lower)) {
            seen.push(lower);
        }
    }
    seen.sort();
    seen
}

#[allow(dead_code)] // 仅供 extract_tag_names 使用
fn collect_tag_names(expr: &Expr, lax_tags: bool, out: &mut Vec<String>) {
    match expr {
        Expr::Tag(t) => out.push(t.clone()),
        Expr::Keyword(k) if lax_tags => out.push(k.clone()),
        Expr::And(l, r) | Expr::Or(l, r) => {
            collect_tag_names(l, lax_tags, out);
            collect_tag_names(r, lax_tags, out);
        }
        Expr::Not(inner) => collect_tag_names(inner, lax_tags, out),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_keyword() {
        let e = parse_query("rust").unwrap();
        assert_eq!(e, Expr::Keyword("rust".into()));
    }

    #[test]
    fn parses_phrase() {
        let e = parse_query("\"hello world\"").unwrap();
        assert_eq!(e, Expr::Phrase("hello world".into()));
    }

    #[test]
    fn implicit_and() {
        let e = parse_query("rust web").unwrap();
        assert_eq!(
            e,
            Expr::And(
                Box::new(Expr::Keyword("rust".into())),
                Box::new(Expr::Keyword("web".into()))
            )
        );
    }

    #[test]
    fn explicit_and_or_not() {
        let e = parse_query("rust AND web OR NOT unread").unwrap();
        // (rust AND web) OR (NOT unread)
        assert_eq!(
            e,
            Expr::Or(
                Box::new(Expr::And(
                    Box::new(Expr::Keyword("rust".into())),
                    Box::new(Expr::Keyword("web".into()))
                )),
                Box::new(Expr::Not(Box::new(Expr::Unread)))
            )
        );
    }

    #[test]
    fn parens_grouping() {
        let e = parse_query("(rust OR web) AND NOT unread").unwrap();
        assert!(matches!(e, Expr::And(_, ref inner) if matches!(**inner, Expr::Not(_))));
    }

    #[test]
    fn tag_prefix() {
        let e = parse_query("tag:rust").unwrap();
        assert_eq!(e, Expr::Tag("rust".into()));
    }

    #[test]
    fn bang_untagged_keyword() {
        let e = parse_query("!untagged").unwrap();
        assert_eq!(e, Expr::Untagged);
    }

    #[test]
    fn bang_unread_keyword() {
        let e = parse_query("!unread").unwrap();
        assert_eq!(e, Expr::Unread);
    }
    #[test]
    fn untagged_keyword() {
        let e = parse_query("untagged").unwrap();
        assert_eq!(e, Expr::Untagged);
    }

    #[test]
    fn unclosed_quote_is_error() {
        assert!(parse_query("\"open phrase").is_err());
    }

    #[test]
    fn missing_rparen_is_error() {
        assert!(parse_query("(rust").is_err());
    }

    #[test]
    fn trailing_rparen_is_error() {
        assert!(parse_query("rust)").is_err());
    }

    #[test]
    fn empty_tag_is_error() {
        assert!(parse_query("tag:").is_err());
    }

    #[test]
    fn empty_query_is_error() {
        assert!(parse_query("   ").is_err());
    }

    #[test]
    fn compile_binds_and_or() {
        let e = parse_query("rust AND tag:web OR untagged").unwrap();
        let mut binds = Vec::new();
        let sql = compile(&e, &mut binds);
        assert!(sql.contains("LOWER(b.title) LIKE ?"));
        assert!(sql.contains("LOWER(b.url) LIKE ?"));
        assert!(sql.contains("LOWER(tg.name) = ?"));
        assert!(sql.contains("NOT IN"));
        assert_eq!(binds.len(), 5);
        assert_eq!(binds[0], "%rust%");
        assert_eq!(binds[4], "web");
    }

    #[test]
    fn compile_unread_with_tag() {
        let e = parse_query("#下载").unwrap();
        let mut binds = Vec::new();
        let sql = compile(&Expr::And(Box::new(Expr::Unread), Box::new(e)), &mut binds);
        assert!(sql.contains("b.is_unread = TRUE"), "sql: {sql}");
        assert!(sql.contains("LOWER(tg.name)"), "sql: {sql}");
        assert_eq!(binds, vec!["下载".to_string()]);
    }
}
