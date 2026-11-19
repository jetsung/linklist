# SPEC.md

本文件保存本仓库的开发规范，约束所有参与开发的 AI 助手与自动化代理。修改代码前必须先阅读并遵守本文件（经 AGENTS.md 引用加载）。

## 书签列表项 actions 区规范（强制）

所有 `/bookmarks/*` 页面（含 `/bookmarks`、`/bookmarks/archived`、`/bookmarks/shared`、`/bookmarks/public-shared` 等全部相关页面）中，每条书签下方的 actions 区必须按以下顺序渲染：

```
日期 | View Edit Archive(或 Unarchive) Remove | Unread Shared
```
1. **三组结构（强制）**：整个 actions 区只允许三个组，组间用且仅用一个 `|` 分隔，共 1–2 个 `|`：
   - 第 1 组：日期（单项）。
   - 第 2 组：View、Edit、Archive/Unarchive、Remove 依次排列。
   - 第 3 组：extra-actions（Unread、Shared），按条件渲染：`is_unread` 为真显示 Unread，`is_shared` 为真显示 Shared；两者都无时不渲染第 3 组及第 2 个 `|`。
   - 禁止出现第四组；禁止在组内再插入额外的 `|`。
2. **Archive 与 Unarchive 二选一**：未归档显示 Archive，已归档显示 Unarchive，不得同时出现。
3. **共享页例外**：`/bookmarks/shared` 上他人共享的书签为只读，只显示 `日期 | View Shared by <user>`，不显示 Edit/Archive/Remove/Unread/Shared；本人共享的书签仍按第 1 条完整三组结构渲染（Edit 带 `return_url=/bookmarks/shared`）。
4. **实现位置**：统一在 `frontend/astro/src/scripts/bookmarks-page.ts` 的 `renderBookmarkItem` 中实现；任何页面不得另起一套 actions 渲染逻辑。新增书签列表页面时必须复用该函数。
5. **验证**：改动 actions 区后，必须用真实构建产物（`pnpm build` 后的 `frontend/dist`）做 DOM 级断言，逐页核对三组结构与 `|` 数量，不得只靠目测。

违反以上任何一条都视为回归，必须修复后才能提交。

## 搜索与标签云规范（强制）

所有书签列表页（未归档、已归档、未读、共享、公开共享、无标签、bundle）共用同一套搜索/标签云行为，实现统一位于 `frontend/astro/src/scripts/bookmarks-page.ts` 与后端 `src/web/bookmarks_api.rs`、`src/web/shared_api.rs`。

1. **q 整串透传**：前端不得在前端拆解 `q`；`state.q` 原样作为 `q` 参数传给列表接口与标签统计接口，由后端 `bookmarks::search::parse_query` 统一解析（`#标签`、`!untagged`、`tag:`、AND/OR/NOT、短语）。
2. **多标签 AND 组合**：`?q=#A1 #A2` 表示同时满足两个标签（AND），与 `/api/v1/bookmarks` 同一套表达式语义。多个 `#标签` 之间为隐式 AND；非 `#` 内容为关键词。
3. **标签云 selected 区**：查询中的每个 `#标签` 逐个渲染移除链接（`-标签名`），链接 = 当前页面参数保留、从 q 中去掉该标签。
4. **标签云 unselected 区**：渲染当前筛选结果中共现的标签，点击 = 在现有 q 后追加 `#标签`（继续 AND 组合），不得切换/覆盖已有标签。
5. **栏目参数保留（强制）**：标签云所有链接（追加/移除）必须继承当前页面 URL 的固有查询参数（如 `unread=yes`、`shared=yes`、`user=`、`bundle=N`），仅替换 `q`、删除 `page`。示例：未读页 `/bookmarks?unread=yes` 点 AI 后为 `/bookmarks?unread=yes&q=%23AI`。
6. **共享接口匿名语义**：`/api/v1/shared`、`/api/v1/shared/tags`、`/api/v1/shared/users` 匿名可访问；登录 = 全部开启共享用户的共享书签，匿名 = 仅 `enable_public_sharing` 用户的公开共享视图。
7. **标签云排序（强制）**：所有标签云/标签列表接口与页面（`/api/v1/bookmarks/tags`、`/api/v1/bookmarks` 查询标签、`/api/v1/shared/tags`、`/api/v1/shared/public/tags`、`/api/v1/bundles/preview/tags` 等）必须统一按：英文字母 A–Z 在前、中文按拼音 A–Z 在后排序；不得按使用次数排序。实现统一复用 `crate::bookmarks::tags::sort_tags_english_first`。

违反以上任何一条都视为回归，必须修复后才能提交。

## 确认与提示交互规范（强制）

前端所有需要用户确认或反馈的场景，禁止使用原生 `alert`、`confirm`、`prompt`：

1. **确认操作**（删除等破坏性动作前）必须使用 `frontend/astro/src/scripts/confirm.ts` 的 `confirmDialog(message)` 模态确认（`.modal.modal-confirm`，返回 `Promise<boolean>`）。
2. **操作结果提示**必须使用 `frontend/astro/src/scripts/toast.ts` 的 `toastSuccess` / `toastError`（破坏性操作完成提示传 `{ blocking: true }`）。
3. 新增页面/组件不得引入任何原生 `alert`/`confirm`/`prompt` 调用；代码评审与验证时按此检查。

违反以上任何一条都视为回归，必须修复后才能提交。

## Bundle 编辑页与 Preview 规范（强制）

Bundle 的查看/编辑入口是独立页面 `/bundles/{id}/edit`（左表单 + 右 Preview），列表页上的"编辑"必须是跳转该页的链接，不得回退为模态编辑；模态仅用于新建。

1. **页面结构**：`frontend/astro/src/pages/bundles/edit.astro`，根元素 `.bundles-editor-page`；`<main>` 内表单字段 id 固定为 `bundle-name`、`bundle-q`、`bundle-any-tags`、`bundle-all-tags`、`bundle-excluded-tags`、`bundle-filter-unread`、`bundle-filter-shared`，底部 `.form-footer` 含 `#bundle-save` 与指向 `/bundles` 的取消链接；`<aside class="col-2">` 内 `#bundle-preview` 渲染 Preview。
2. **动态路由**：Astro 静态构建产物为 `bundles/edit/index.html`，由 Rust 侧 `frontend::bundles_edit_page` 服务任意 `/bundles/{id}/edit`；前端脚本从 `location.pathname` 提取 bundle id，非法 id 跳回 `/bundles`。
3. **回填与保存**：加载 `GET /api/v1/bundles/{id}` 回填表单（`search_json` 的 q/tags_and/tags_or/tags_not/filter/shared 映射到对应字段，filter unread/read、shared true/false → yes/no）；保存用 `PATCH /api/v1/bundles/{id}`，载荷为 `{ name, search_json: {...} }`（后端同时兼容 `search` 字段）；成功后 `queueToast('Bundle 已更新')` 并跳转 `/bundles`；401 → `/login`。
4. **Preview 接口**：预览数据一律来自 `GET /api/v1/bundles/preview`（表单参数 `search/any_tags/all_tags/excluded_tags/filter_unread/filter_shared/page/per_page`），后端以 `filter: "active"` + 表单过滤条件构造查询，返回 `{total, page, per_page, results}`，results 每项附带 `tags`；标签统计用 `GET /api/v1/bundles/preview/tags`。新增预览相关能力必须扩展这两个接口，不得绕开另起查询。
5. **实时刷新**：表单 input 事件 500ms 防抖后刷新 Preview（页码重置为 1）；Preview 含中文计数（"找到 N 个匹配此 Bundle 的书签"）、简化书签条目（favicon、标题链接、URL、标签、绝对日期）与上一页/页码/下一页分页。
6. **items_per_page**：预览每页条数取 `GET /api/v1/profile` 的 `items_per_page`（clamp 1–200，默认 30）。
7. **验证**：改动本页或 preview 接口后，必须 `pnpm build` 后用 jsdom 加载 `frontend/dist` 真实产物断言回填、预览渲染与 PATCH 载荷（脚本需 IIFE 隔离注入 api/toast 依赖），并用 curl 实测 preview 接口的过滤/分页/鉴权；`cargo test` 全量通过。

违反以上任何一条都视为回归，必须修复后才能提交。
