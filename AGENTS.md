# AGENTS.md

本文件约束所有参与本仓库开发的 AI 助手与自动化代理。修改代码前必须先阅读并遵守本文件。

仓库开发规范统一保存在 @SPEC.md 中，必须完整阅读并严格遵守。

## 数据库支持

本项目必须同时支持三种数据库：**sqlite（默认）/ postgres / mysql**。

- 数据库连接一律通过环境变量 `DATABASE_URL` 配置，例如：
  - sqlite（默认）：`sqlite://data/linklist.db`（未设置 `DATABASE_URL` 时使用默认 sqlite）
  - postgres：`postgres://user:password@host:5432/dbname`
  - mysql：`mysql://user:password@host:3306/dbname`
- 任何涉及数据库的代码变更（SQL 语句、迁移脚本、ORM 用法等）都必须保证在三种数据库下均可正确运行；禁止使用仅单一数据库支持的方言特性，除非已为其余数据库提供等价实现。
- 新增迁移时必须同时提供三种数据库对应的迁移文件。
前端交互禁止使用原生 `alert`/`confirm`/`prompt` 作为确认或提示，一律使用项目统一的 `toast`（`src/scripts/toast.ts`）与 `confirmDialog`（`src/scripts/confirm.ts`）模态确认。
具体强制规范见 @SPEC.md。
