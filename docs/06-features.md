# 功能使用

## 书签与标签

- **创建**：粘贴 URL，自动抓取标题/描述（后台任务），可选填备注（Markdown）。
- **URL 判重**：同一用户下 URL 归一化后判重，重复创建会合并/提示。
- **标签**：创建时输入或事后编辑；`PUT /bookmarks/{id}/tags` 全量替换。
- **批量操作**：多选后批量加/删标签、标记已读/未读、删除。
- **归档**：归档后从主列表移出，可随时恢复或彻底删除。

## 搜索表达式

搜索框支持上游（linkding）风格的搜索表达式：

| 语法 | 含义 |
|---|---|
| `rust` | 关键词：标题/描述/备注/URL 大小写不敏感的包含匹配 |
| `"exact phrase"` | 短语：四列包含匹配，短语不再二次分词 |
| `tag:rust` / `#rust` | 限定标签（`#` 支持中文标签名，多个为 AND 组合） |
| `is:unread` / `unread` | 仅未读 |
| `untagged` | 无标签书签 |
| `a AND b` / `a OR b` / `NOT a` | 组合表达式 |
| `-word` / `!word` | 排除词 |

普通关键词在 lax 模式下会额外命中标签名（用户偏好
`tag_search = lax`，默认 `strict`）。

## 自动打标

在设置中保存自动打标规则（`PUT /api/v1/profile/auto-tagging-rules`），
创建书签时按规则命中标题/URL 自动附加标签。

## 快照与归档

- **网页快照**：启用 `enable_snapshots` 后可抓取 singlefile 本地快照，
  在详情页查看；也可手动触发
  `POST /bookmarks/{id}/capture`（`kind = snapshot`）。
- **Internet Archive 回查**：开启 `web_archive_integration` 后，创建书签
  追加 archive_lookup 任务，可用时回填 `archive_url`。
- **资产上传**：`POST /bookmarks/{id}/assets`（multipart，类型白名单 +
  大小限制）。
- **预览图**：抓取 Open Graph 图片作预览（用户偏好 `enable_preview_images`）。

## 共享书签

- 登录用户互相可见开启共享的用户的共享书签（只读他人、可操作自己的）。
- 匿名公开共享：管理员开启用户资料中的 `enable_public_sharing`，
  且实例开启 `guest_sharing` 后，匿名访客可访问公开共享视图与 RSS
  订阅源（`GET /api/v1/feed/public_shared[?key=...]`）。

## 用户偏好

每用户资料（`GET/PUT /api/v1/profile`）覆盖：主题、每页条数、标签分组、
favicon/预览图/快照/共享开关、日期显示、操作按钮显隐、搜索偏好（保存为
默认后内容列表按偏好刷新）等，对齐 linkding v1.47.0 的设置项。

## 导入导出

```bash
# Netscape 格式（浏览器书签互转）
curl -X POST -H "Authorization: Token lk_..." \
  -F "file=@bookmarks.html" http://localhost:9090/api/v1/bookmarks/import

curl -H "Authorization: Token lk_..." \
  http://localhost:9090/api/v1/bookmarks/export -o bookmarks.html
```

导入时重复 URL 判重合并（标题/描述/未读/日期覆盖策略与上游一致）。
