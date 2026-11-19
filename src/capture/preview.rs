use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::capture::http::{outbound_client, safe_get};

/// 预览图目录：{data_dir}/previews（对齐上游 LD_PREVIEW_FOLDER=data/previews）。
pub fn previews_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("previews")
}

/// 允许的图片扩展名（对齐上游 LD_PREVIEW_ALLOWED_EXTENSIONS）。
const ALLOWED_EXTENSIONS: &[&str] = &[".jpg", ".jpeg", ".png", ".gif", ".svg", ".webp"];

/// Content-Type → 扩展名（对齐上游 mimetypes.guess_extension + 白名单校验）。
fn extension_for_content_type(content_type: &str) -> Option<&'static str> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let ext = match mime.as_str() {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/svg+xml" => ".svg",
        "image/webp" => ".webp",
        _ => return None,
    };
    ALLOWED_EXTENSIONS.iter().find(|e| **e == ext).copied()
}

/// 预览图文件名：sha256(书签 URL) hex + 扩展名。
/// 上游用 md5(书签 URL)，这里沿用项目已有的 sha2 依赖，仅哈希算法不同。
pub fn preview_file_name(bookmark_url: &str, ext: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bookmark_url.as_bytes());
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{hex}{ext}")
}

/// 下载结果：文件名 + 图片字节。
pub struct DownloadedPreview {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// 抓取页面元数据中的 og:image 并下载（对齐上游 load_preview_image）。
/// 任一步骤失败返回 Err（调用方记录后放弃，不算致命）。
pub async fn download_preview(bookmark_url: &str) -> Result<DownloadedPreview, String> {
    // 1. 抓页面元数据拿 og:image（走缓存）。
    let meta = crate::capture::metadata::fetch_metadata_cached(bookmark_url).await?;
    let Some(image_url) = meta.preview_image_url else {
        return Err("页面元数据无预览图（og:image）".into());
    };

    // 2. 下载图片（SSRF 校验 + 2MiB 上限复用 safe_get）。
    let client = outbound_client();
    let (_url, bytes, headers) = safe_get(&client, &image_url).await?;

    // 3. Content-Type 白名单校验。
    let content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(ext) = extension_for_content_type(content_type) else {
        return Err(format!("不支持的图片类型: {content_type}"));
    };

    Ok(DownloadedPreview {
        file_name: preview_file_name(bookmark_url, ext),
        bytes,
    })
}

/// 下载并写入 {data_dir}/previews/{sha256}{ext}，返回文件名。
pub async fn save_preview(data_dir: &Path, bookmark_url: &str) -> Result<String, String> {
    let downloaded = download_preview(bookmark_url).await?;
    let dir = previews_dir(data_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("创建预览图目录失败: {e}"))?;
    let path = dir.join(&downloaded.file_name);
    tokio::fs::write(&path, &downloaded.bytes)
        .await
        .map_err(|e| format!("写预览图失败: {e}"))?;
    Ok(downloaded.file_name)
}

/// 由文件名推断图片 Content-Type（服务端点用）。
pub fn content_type_for_file_name(file_name: &str) -> &'static str {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        // .jpg / .jpeg 及未知扩展兜底。
        "image/jpeg"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_mapping_whitelist() {
        assert_eq!(extension_for_content_type("image/jpeg"), Some(".jpg"));
        assert_eq!(
            extension_for_content_type("image/png; charset=binary"),
            Some(".png")
        );
        assert_eq!(extension_for_content_type("image/svg+xml"), Some(".svg"));
        assert_eq!(extension_for_content_type("image/webp"), Some(".webp"));
        // 非图片 / 未收录类型一律拒绝。
        assert_eq!(extension_for_content_type("text/html"), None);
        assert_eq!(extension_for_content_type("image/bmp"), None);
        assert_eq!(extension_for_content_type(""), None);
    }

    #[test]
    fn file_name_is_deterministic_sha256() {
        let a = preview_file_name("https://example.com/a", ".png");
        let b = preview_file_name("https://example.com/a", ".png");
        let c = preview_file_name("https://example.com/b", ".png");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.ends_with(".png"));
        assert_eq!(a.len(), 64 + 4);
    }

    #[test]
    fn content_type_roundtrip() {
        assert_eq!(content_type_for_file_name("abc.png"), "image/png");
        assert_eq!(content_type_for_file_name("abc.svg"), "image/svg+xml");
        assert_eq!(content_type_for_file_name("abc.jpg"), "image/jpeg");
        assert_eq!(content_type_for_file_name("abc.jpeg"), "image/jpeg");
        assert_eq!(content_type_for_file_name("abc.webp"), "image/webp");
    }
}
