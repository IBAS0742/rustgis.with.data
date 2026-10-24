# Prompt 03 — Release Readiness

请基于当前 checkout 和实际测试结果进行发布前检查。不要推断测试通过。

## 必须验证

1. 当前 commit、工作区状态、版本号。
2. RustGIS 仓库规定的 CI 命令是否通过。
3. 本资料包 `docs/ACCEPTANCE_TESTS.md` 中每项状态：PASS / FAIL / NOT RUN。
4. 工程版本不可覆盖是否同时在 UI 与服务端强制。
5. 文件夹配置和工程版本在进程重启/容器重建后是否存在。
6. 路径访问是否限制在配置根目录；默认网络绑定是否安全。
7. PostgreSQL 密码是否可能出现在工程、日志、URL 或 API。
8. 10 GB 非 COG TIFF 测试的实际内存、首屏时间和错误情况；若未测，明确标记 NOT RUN。
9. Windows、Linux、WSL、Docker 各环境的真实构建/运行结果。
10. README、配置样例、迁移说明、备份/恢复说明是否完整。

## 输出

- 发布结论：READY / READY WITH LIMITATIONS / NOT READY。
- 测试矩阵和证据。
- 已知限制。
- 数据目录与持久化目录的备份/恢复步骤。
- 首次部署步骤。
- 不应暴露公网的安全提醒（如果没有认证）。
- 明确未完成事项。

不得将未运行的手动测试写成通过；不得把目标性能写成实测性能。
