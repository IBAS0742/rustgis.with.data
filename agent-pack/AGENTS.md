# Agent Instructions — RustGIS Lightweight Preview

## Mission

Modify the checked-out RustGIS repository into a small, self-hosted GIS preview web application. Reuse RustGIS's existing map, layer, renderer, data providers, and `.rgis` project logic wherever possible. Add only the server-side capabilities and project-version safeguards that are missing.

Read these files before coding:
1. `docs/PRODUCT_SPEC.md`
2. `docs/ARCHITECTURE.md`
3. `docs/IMPLEMENTATION_PLAN.md`
4. `docs/ACCEPTANCE_TESTS.md`
5. `docs/REPO_FACTS_AND_REFERENCES.md`

## Mandatory workflow

- First perform a read-only repository audit. Do not edit files during the audit.
- Identify the current Git commit, working tree state, Rust toolchain, workspace crates, web app entry point, project serialization/open/save code, source/provider abstractions, PostGIS implementation, and existing tests.
- Cite exact file paths and symbols in the audit. Never invent paths, functions, or APIs.
- Present a concise plan and list of expected changed files before implementation.
- Implement one phase at a time. After each phase, run the narrowest relevant tests, then broader checks where feasible.
- Review `git diff` after each phase. Do not overwrite unrelated user changes.
- If a command fails, report the actual failure. Do not claim tests passed unless they were run and passed.
- Do not delete, skip, weaken, or rewrite unrelated tests just to make CI pass.
- Prefer small diffs and existing conventions over broad refactors.
- Ask for clarification only when blocked by a genuinely ambiguous product decision. Otherwise choose the simplest safe implementation and document it.

## 代码提交工作流(每个 AI 修改完成后必须执行)

本项目的 git 提交由用户双击一次性脚本完成,AI 不直接执行 git commit/push。修改完成并通过相关测试后,必须:

1. 在仓库根目录 `RustGIS/` 生成提交脚本,命名为 `NNN-<简短英文标识>-gitcommit.bat`:
   - `NNN` 为三位递增序号:取仓库根目录现有 `NNN-*-gitcommit.bat` 的最大序号加一;没有则从 `001` 开始。
   - 复制 `RustGIS/agent-pack/gitcommit-template.bat` 生成,把 `__COMMIT_MSG__` 替换为本阶段的真实提交信息;不要改动脚本其他结构。
2. 提交信息必须单行,且不得包含 `%` `!` `&` `|` `<` `>` `^` 等批处理特殊字符。
3. 脚本行为(模板已实现):`git add -A` → `git commit` → `git push -u origin HEAD`;任一步失败会保留脚本并提示错误;全部成功后删除自身。再次双击会自动跳过已完成的步骤,因此失败后修复配置再双击即可续跑。
4. 这些脚本已被仓库 `.gitignore` 忽略(`/[0-9][0-9][0-9]-*-gitcommit.bat`),不会进入提交内容。
5. 在最终回复中告知用户双击哪个脚本完成提交;若因缺少 git 身份(user.name/user.email)或可推送的 origin 远程而失败,按脚本提示配置后重新双击。

## RustGIS-specific safety constraints

- Follow the repository's own `AGENTS.md`, contribution guide, lint policy, and test commands. The checked-out repository instructions take precedence if they are stricter.
- Never add `unwrap()`, `expect()`, `panic!`, `unreachable!`, or `todo!` to production code if repository policy forbids them.
- Avoid `unsafe`; follow the crate's existing `unsafe_code` policy.
- Treat paths, project files, API inputs, database metadata, and filenames as untrusted input.
- Do not use a path supplied by a browser directly without canonicalization and allowlist validation.
- Do not expose arbitrary filesystem read/write access through HTTP.
- Never put database passwords, access tokens, or connection secrets into project JSON, logs, error messages, URLs, or client-visible responses.
- Do not execute shell commands using user-controlled strings.
- Do not load a large raster fully into memory merely to render a preview. Reuse existing overviews/windowed reads; if unsupported, propose a bounded preprocessing path.
- Avoid introducing a new GIS rendering engine, tile server, authentication system, or database unless the audit proves it is necessary.

## Product invariants

1. Data sources and project files persist across process restarts.
2. Source roots can be added and rescanned. Newly discovered files are browseable but are not silently added to a project.
3. A saved project version is immutable through the application API.
4. Editing happens in a working copy created from a saved version. Saving creates a new unique version; it never overwrites an existing version.
5. Missing/unreachable layers are marked unavailable and are not rendered. Their project records should be preserved when safely possible so they can recover when data returns.
6. A project stores references and presentation state, not a copy of all source data, except where RustGIS's existing project format inherently embeds data. Document this behavior accurately.
7. No user accounts, roles, or collaborative editing are in scope.
8. Support Windows, Linux, WSL, and Docker through configuration and path handling; do not assume all path formats are interchangeable.
9. Do not claim support for a platform until build/run instructions and tests or documented manual checks exist.

## Definition of done for any phase

- Code follows repository conventions.
- Relevant tests added or updated.
- Tests run and results reported truthfully.
- API and data behavior documented.
- No secrets are committed.
- No unrelated files changed.
- The final response lists changed files, tests executed, known limitations, and next steps.
