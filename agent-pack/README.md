# RustGIS 轻量内网预览改造 — Agent 资料包

本资料包用于直接提供给代码 Agent（例如 DeepSeek-V4.1-Flash），指导其在 RustGIS 现有仓库上进行小步、可测试的改造。

## 使用方法

1. 将本资料包解压到 RustGIS 仓库根目录；如不想覆盖仓库已有文件，可只复制 `AGENTS.md` 和 `docs/`、`prompts/`。
2. **先运行 `prompts/00_REPOSITORY_AUDIT.md`，只读审计，不允许修改代码。**
3. 阅读审计报告，确认 Agent 识别了真实的 Web 入口、工程读写路径、数据源接口和测试命令。
4. 再按 `docs/IMPLEMENTATION_PLAN.md` 分阶段实施，每阶段独立提交或形成可审查的 diff。
5. 所有功能以 `docs/ACCEPTANCE_TESTS.md` 为验收标准。不能因为测试难通过而删除或放宽测试。

## 资料包文件

- `AGENTS.md`：给代码 Agent 的最高优先级项目约束。
- `docs/PRODUCT_SPEC.md`：目标产品与非目标范围。
- `docs/ARCHITECTURE.md`：建议架构、数据模型、安全边界。
- `docs/IMPLEMENTATION_PLAN.md`：阶段拆分与完成定义。
- `docs/ACCEPTANCE_TESTS.md`：验收用例。
- `docs/REPO_FACTS_AND_REFERENCES.md`：已知 RustGIS 事实、待源码核验事项和官方参考链接。
- `prompts/00_REPOSITORY_AUDIT.md`：只读审计提示词。
- `prompts/01_IMPLEMENT_PHASE.md`：按阶段实施提示词。
- `prompts/02_REVIEW_DIFF.md`：代码审查提示词。
- `prompts/03_RELEASE_CHECKLIST.md`：发布前检查提示词。

## 重要原则

这是一个“基于 RustGIS 的轻量改造”项目，不是重写 GIS 引擎。所有仓库路径、API、crate 名称和现有功能都必须先从实际 checkout 中核验；资料包中的架构建议不是声称这些功能已经存在。
