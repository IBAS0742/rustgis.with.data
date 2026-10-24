# 已知问题记录 — Known Issues

按发现时间倒序记录。每条包含：现象、根因、是否已解决、修复步骤。

---

## 2026-10-24 · docs 工作流 Pages 部署失败（404 Not Found）

**状态：待用户在仓库设置中启用 GitHub Pages 后重跑工作流。** 无法通过修改 `.github/` 目录下的任何配置解决。

### 现象

向 `main` 推送（提交 `a40918b`）后自动触发的 `docs` 工作流（`.github/workflows/docs.yml`）失败。`build` 作业成功（文档构建并上传 artifact，日志显示 `Found 1 artifact(s)`），`deploy` 作业在 `actions/deploy-pages@v5` 步骤失败：

```
Error: Creating Pages deployment failed
Error: HttpError: Not Found
Error: Failed to create deployment (status: 404) with build version a40918b0...
Ensure GitHub Pages has been enabled:
https://github.com/IBAS0742/rustgis.with.data/settings/pages
```

### 根因

仓库 `IBAS0742/rustgis.with.data` 尚未启用 GitHub Pages。`actions/deploy-pages` 调用 Pages 部署 API 时，未启用 Pages 的仓库会返回 404。工作流文件本身没有问题：

- `docs.yml` 已正确声明 `permissions: pages: write, id-token: write`、`environment: github-pages`；
- artifact 已成功上传并被 deploy 步骤找到；
- 失败点是「创建 Pages 部署」这一 API 调用，属于仓库级设置缺失，任何 workflow YAML 都无法开启它。

### 修复步骤（需要仓库管理员在 GitHub 网页或通过 API 操作）

方式一（网页）：

1. 打开 https://github.com/IBAS0742/rustgis.with.data/settings/pages ；
2. 在 **Build and deployment → Source** 中选择 **GitHub Actions**（不要选 "Deploy from a branch"）。

方式二（gh CLI，需要对该仓库有 admin 权限的令牌）：

```bash
gh api -X POST repos/IBAS0742/rustgis.with.data/pages -f build_type=workflow
```

### 验证

启用后不需要新提交：在 Actions 页面打开该次失败的 `docs` 运行，点击 **Re-run failed jobs** 重跑 `deploy` 作业即可。部署成功后可通过仓库 Pages URL 访问文档站点。

### 注意事项

- 若该仓库是私有仓库且账号是 GitHub Free 计划，Pages 对私有仓库不可用（设置页会提示升级到 Pro/Team 计划），届时需改用公开仓库或付费计划。
- 不要把任何访问令牌写入本文件、工作流日志或仓库配置。
