# 架构建议

本文件是目标架构建议，不代表仓库当前已经存在这些模块。实施前必须用源码审计确认最合适的接入点。

## 1. 总体结构

- Web 前端：尽量复用 RustGIS `rustgis-web` 的 UI、地图、图层和项目逻辑。
- 轻量服务端：负责已配置目录、文件元数据、受控文件读取、PostgreSQL/PostGIS 连接与浏览、工程版本存储。
- 持久化目录：保存 source registry、project versions、缓存和日志。
- 原始数据：保持原位，后端以只读方式访问。

优先选择 RustGIS 仓库内已有技术栈。若后端必须新增，先说明为什么现有 crate/应用无法承载；不要仅因为熟悉某个框架就引入新栈。

## 2. 建议持久化结构

可以采用类似布局，但必须先确认仓库现有约定：

```text
data-home/
  config/
    sources.json
    database-connections.json   # 不保存明文密码
  projects/
    <project-id>/
      project.json              # 工程元数据/名称
      versions/
        <version-id>.rgis      # 不可覆盖版本
  cache/
  logs/
```

如果已有 `.rgis` 能完整保存地图状态，应继续使用它作为工程内容；外部元数据只存版本信息、父版本、创建时间和稳定的数据源引用。避免维护两套互相矛盾的图层状态。

## 3. 数据源模型建议

文件根目录：
- `id`：稳定 UUID。
- `display_name`：用户可读名称。
- `root_path`：服务器本地绝对路径。
- `enabled`：是否启用。
- `last_scan_at`：上次扫描时间。
- `scan_status` / `last_error`：扫描状态。

数据库连接：
- `id`、`display_name`、`host`、`port`、`username`、`ssl_mode` 等非秘密字段。
- 密码使用环境变量、操作系统凭据存储或受限权限的 secret 文件；如果最小版本暂不实现密码持久化，重启后要求重新输入并明确提示。
- 不将密码放进工程文件、日志、URL 或前端响应。

文件资源标识应尽可能使用 source ID + 相对路径，而不是把任意绝对路径直接作为客户端可提交的读取目标。数据库图层引用应包含 connection ID、database、schema、table/view、geometry column（如需要）和主键/唯一键（如 provider 需要）。

## 4. API 轮廓（仅为建议）

先检查 RustGIS 现有 command/API 机制，优先复用，不要重复实现。

- `GET /api/sources`
- `POST /api/sources/folders`
- `DELETE /api/sources/folders/{id}`
- `POST /api/sources/folders/{id}/scan`
- `GET /api/sources/folders/{id}/items`
- `GET /api/sources/databases`
- `POST /api/sources/databases`
- `GET /api/sources/databases/{id}/catalog`
- `GET /api/projects`
- `POST /api/projects`
- `POST /api/projects/{id}/copies`
- `GET /api/projects/{id}/versions`
- `GET /api/projects/{id}/versions/{version_id}`
- `POST /api/projects/{id}/versions`（创建新版本，拒绝覆盖）
- 可选：受控的 `GET /api/files/{source_id}/{relative_path}`，仅在现有 provider 无法服务端直接读文件时使用。

对所有请求定义请求/响应 schema、错误码和输入限制。不要让浏览器提交任意服务器路径。

## 5. 扫描策略

- 手动扫描是 MVP 默认策略。
- 扫描只枚举已配置目录内的文件；避免默认递归系统目录或无界网络挂载。
- 支持扩展名筛选，但格式最终应由 provider 验证，不能仅凭扩展名断言文件有效。
- 不自动把扫描发现的新文件添加进已打开工程。
- 对大目录采用分页/惰性浏览，避免一次把所有文件元数据塞进浏览器。

## 6. 工程不可变版本策略

推荐采用“工作副本 + 不可变版本”：
1. 打开保存版本时以只读状态加载。
2. 用户选择“复制工程”后创建新的工作副本 ID。
3. 修改只作用于工作副本。
4. 保存时生成新的唯一 version ID。
5. 后端必须拒绝对已存在 version ID 的覆盖；不依赖 UI 的只读按钮。
6. 写入临时文件，完成校验后原子发布版本；失败时清理临时文件，不留下半成品。
7. 可选：对每个版本保存内容哈希，用于检测意外损坏。

需先检查 RustGIS 的命令/撤销/保存机制，避免其自动保存行为绕过版本约束。工程打开后是否可直接编辑，要由前端状态与服务端保存规则共同约束。

## 7. 缺失图层处理

- 打开工程时检查每个外部引用的可用性。
- 不可用层不参与绘制，但尽量保留工程记录。
- 错误原因要区分：路径不存在、访问被拒绝、数据库不可达、database/schema/table/column 不存在、格式解析失败。
- 不要在校验失败时删除图层记录或重写工程文件。
- 若现有 `.rgis` 打开流程会丢弃无效图层，优先在加载前做引用重映射/占位层适配；先做最小实验验证，不要猜测数据结构。

## 8. 栅格读取

- 先用真实 10 GB GeoTIFF 测试当前 RustGIS Web 路径。
- 优先复用现有 GeoTIFF 解析、overview、windowed read 或 COG Range 请求能力。
- 不允许简单把文件读取成一个完整 byte buffer。
- 若需要预处理，做成后台任务并把派生物放在 cache；原始文件保持不变。
- 缓存 key 至少考虑源标识、文件大小、修改时间，必要时加内容指纹与处理参数。
- 多个请求同时触发相同派生任务时应避免重复处理。
- 在没有实测之前，不承诺 10 GB 非 COG TIFF 一定能即时交互显示。

## 9. 跨平台

- 路径使用标准路径 API，避免硬编码 Windows 分隔符。
- 配置目录可通过环境变量指定，如 `RUSTGIS_PREVIEW_HOME`；实际命名先检查仓库惯例。
- Docker 必须将数据目录和持久化目录分别挂载，容器重建后工程仍存在。
- Windows 盘符路径与 Linux/WSL 挂载路径不能无条件互换。
- WSL 中应验证 Linux 路径和 `/mnt/<drive>` 的权限与性能。
- 文件 watcher 非 MVP 必需；先实现手动扫描。
