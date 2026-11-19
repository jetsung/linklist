# 与 linkding 的功能对齐

LinkList 以 linkding v1.47.0 为上游参考。本文说明能力对齐口径、总体结论与
主要差异，供评估选型与了解行为差异时参考。

## 对齐口径

- **能力对齐而非实现一致**：LinkList 是全新 `/api/v1/*` REST 设计，不追求
  Django 路径/表结构一致，只按"上游能做到的，本项目是否也能做到"评判。
- 上游已废弃/空壳的能力不计入缺口。

## 总体结论

- **核心能力已对齐**：认证与账户、书签 CRUD、列表/搜索/排序/分页、标签、
  批量操作、Netscape 导入/导出、Bundle、共享、RSS、快照与资产、偏好设置、
  系统端点（health/version/opensearch/manifest）均具备可用实现。
- **历史缺口已全部补齐**（导出保真度、singlefile 上传、metadata 缓存、
  bulk/snapshot、导入私有标记与合并、资产安全响应头、标签大小写不敏感、
  共享门控、Bundle 阅读过滤、更新时自动打标、搜索失败空结果、Archive 回查
  消费、OIDC/反代/CORS）。
- **少数差异属有意架构重设计**（见下文），少数为微小行为差异。
- **本项目另有 13+ 项扩展**（见文末清单）。

## 能力域对照

| 能力域 | 状态 | 说明 |
|---|---|---|
| 认证与账户 | ✅ | 登录/登出/改密/首次初始化对齐；OIDC（PKCE + JWKS）、反向代理认证、CORS 对齐 |
| 书签 CRUD | ✅ | 创建/读取/更新/删除、创建时抓取、更新判重对齐；**重复 URL 返回 409**（上游为静默更新）；URL 判重用规范化 + sha256（去跟踪参数/fragment） |
| 列表/搜索/排序 | ✅ | 筛选、六种排序、`modified_since`/`added_since`、搜索表达式（`#tag`/AND/OR/NOT/短语）同套语义；关键词搜索为大小写不敏感包含匹配（三数据库通用实现）；lax 标签兼查已支持 |
| 标签 | ✅ | 列表/重命名/合并对齐；**服务端前缀补全**（上游为前端拉全量过滤） |
| 批量操作 | ✅ | 11 种动作对齐（含批量快照）；**逐条成功/失败结果返回**（上游无） |
| 导入 / 导出 | ✅ | Netscape 保真度对齐：ADD_DATE/LAST_MODIFIED/PRIVATE/TOREAD、归档伪标签、备注块、含已归档书签 |
| Bundle | ✅ | CRUD/预览/标签统计对齐；新增拖拽排序与按 Bundle 拉取书签端点 |
| 共享 | ✅ | 登录共享视图（`enable_sharing` 门控）+ 匿名公开共享（`enable_public_sharing` 门控）双层对齐 |
| RSS 订阅 | ✅ | all/unread/shared/recent 四种源、limit 语义对齐 |
| 快照与资产 | ✅ | 浏览器端 singlefile 上传（gzip）、资产类型白名单与大小限制、内联查看安全头（Content-Disposition + CSP）对齐；快照生成为自研内联实现（上游用 single-file 子进程） |
| 后台任务 | ✅ | metadata/快照/Archive 回查/预览图任务对齐；自研进程内队列 + 指数退避重试（上游为 Huey）；favicon 改为**运行时多源代理 + 双层缓存**（上游为落盘任务） |
| Internet Archive | ⚠️ | 回查链接由任务查 availability API 回填（可能为空）；上游为"永真回退"拼接链接。偏好 `web_archive_integration` 开启时 create/update 自动入队 |
| 系统端点 | ✅ | health/version/opensearch/manifest 对齐 |
| 网络与安全 | ✅ | SSRF 防护（内网白名单）、抓取超时/大小/重定向限制；审计日志、登录限速为扩展 |

## 本项目扩展项（上游没有）

- 管理端 REST：用户增删/启停/重置密码、全量书签、任务列表、全局设置、审计日志查询
- 审计日志（登录/登出/口令变更/Token/setup/用户管理/设置变更等事件）
- 登录失败限速（默认 10 次 / 300 秒，可配置）
- API Token 安全增强：sha256 落库、有效期（TTL）、最后使用时间、可撤销
- URL 规范化强化 + `url_hash` 判重
- 抓取任务显式化（`/capture` 触发 + `/capture-tasks/{id}/retry` 重试）
- 标签服务端前缀补全、标签云中英双语排序规范
- favicon 多源代理 + 内存 LRU + 数据库双层缓存
- 批量操作逐条结果返回
- 三数据库后端（SQLite/PostgreSQL/MySQL）与 `DATABASE_URL` 连接串
- 单二进制交付（前端静态资源编译期嵌入）
