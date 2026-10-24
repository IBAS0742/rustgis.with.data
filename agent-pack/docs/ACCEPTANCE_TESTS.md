# 验收测试清单

测试结果必须记录为 PASS / FAIL / NOT RUN，附命令、环境和必要日志。不得用推测代替测试。

## 执行环境与结论

- 日期：2026-10-23/24。
- 代码位置：WSL2 Ubuntu 中的 `/home/ubuntu/RustGIS`；基线 commit `23d5ba3f75ed2ae79bf3168cd6f50e70245bc8f5`。所有修改仍未提交。
- 工具链：Rust `1.99.1`，用户空间 Zig `0.17.0` 包装器 `/home/ubuntu/.local/bin/cc`。
- 最终完整检查：`RUST_TEST_THREADS=1 cargo xtask ci` **PASS**（fmt、workspace clippy、workspace tests、layers 全部通过）；2026-10-24 在 COG tile availability、TIFF 上限和保存可见性改动后再次执行同一命令，仍为 **PASS**。
- 默认并行执行 `cargo xtask ci` 在本机曾间歇出现既有 `rustgis-ui-egui` EGL 测试 SIGSEGV；单独运行 `cargo test -p rustgis-ui-egui --lib -q` 通过，串行完整检查通过。预览相关测试没有失败。
- Docker：Docker `29.8.2`；`docker compose config --quiet` PASS；最终 Dockerfile（默认 BuildKit frontend，Cargo registry/git/target cache mount）构建 PASS。2026-10-24 最新构建镜像 `rustgis-preview:local` 的 manifest list 为 `sha256:d2064f373952827a930b4dd70d25fb81cffe003eaf288ff7968741e665d5bf83`，259,469,454 bytes；容器健康检查、`/`、`/map.html`、源浏览、Range、命名卷持久化和只读源目录挂载 PASS。最新镜像的 `/console.js` 已确认包含 64 MiB TIFF 上限和 COG tile 可见性恢复逻辑。本机 Docker Hub 直连不可用，基础镜像已预先从镜像站拉取并 tag 为官方名称；Dockerfile 仍引用官方镜像名。
- 重要说明：验收末段 WSL 重启清空了 `/tmp` 测试数据。缺失图层恢复、PostGIS 详细场景来自重启前的实测记录；最终二进制随后在新建 home 上复测了创建 V1、只读限制、工作副本、添加 GeoJSON、保存 V2、Recheck、Range 和文件 API 方法限制。
- 2026-10-24 补充：用验收辅助 Rust example（`crates/data/examples/make_preview_geotiff.rs`、`make_preview_cog.rs`）生成 486-byte、16×16、Float32、Deflate 的小型非 COG GeoTIFF，并在最终控制台完成“添加→渲染→保存 V2→只读重开”实测；同时生成 1,377,006-byte 的 COG，确认它被保存为 `maps[].tiles` 的 URL 流式图层，并验证了删除源文件→不可用/隐藏、恢复文件→可用/可见、以及隐藏状态下保存工作副本不会把 `visible:false` 写进新版本。非 COG TIFF 会被整文件下载并以 embedded bytes 写入工程，因此源浏览器新增了 64 MiB TIFF 上限；大 TIFF 性能仍未验证。
- 2026-10-24 交互重构与中文化（浏览器自动化实测，数据目录 `/home/ubuntu/rustgis-preview/home`）：新建工程现在直接创建并打开工作副本（服务端仍保留空 V1 作为基线）；未打开工程时点击“添加图层”会自动创建“未命名工程”并进入工作副本；在只读版本上添加图层会自动复制出工作副本。控制台界面与图层可用性 reason 全部中文化（status 枚举值保留英文作为 API 稳定值，由前端映射为中文标签）。实测路径：无工程直接添加 `points.geojson` → 自动建工程并进入工作副本 → 保存发布新版本（2365 字节，含 1 图层）→ 只读重开确认“只读版本”徽标、保存按钮禁用、复制按钮可用、reason 显示中文。`cargo test -p rustgis-preview`（35 项）与 `cargo clippy -p rustgis-preview --all-targets` 通过；`server.rs` 控制台标记断言与 `availability.rs` 测试的 reason 断言随中文化同步更新。已知限制：嵌入式地图（`apps/rustgis-web` 上游 WASM 应用）自身界面仍为英文（上游无 i18n 机制）；`error.rs` 的 API 错误消息仍为英文。

## A. 工程版本

- [x] 创建空工程并保存为版本 V1。 — **PASS**（最终控制台创建 `prj_86a0d621f0d4280a28550d2c9b14fe57` / `ver_79df075c9c0dc145333a7f2566728be1`，V1 为 1166 bytes、0 图层。）
- [x] 关闭并重启服务后，V1 仍可打开。 — **PASS**（`first_version_survives_restart`；此前完整 E2E 也重启服务后打开 V1。）
- [x] V1 只读状态下，UI 不允许直接保存覆盖。 — **PASS**（最终控制台打开 V1 后显示 `read-only version`，`Save new version` 为 disabled；服务端对版本写请求返回 `409 version_immutable`。）
- [x] 直接调用 API 覆盖 V1 的版本 ID，服务端拒绝（冲突状态码或明确等效错误）。 — **PASS**（`immutable_route_is_a_conflict` 返回 409；`publishing_an_existing_version_id_is_refused_and_does_not_replace_it` 验证原内容不变。）
- [x] 复制 V1 为工作副本，修改图层顺序/可见性/样式并保存为 V2。 — **PASS**（最终控制台复制为 `wrk_878b9aa6bb1f6ef50e4bad9894bc2c87`，添加 GeoJSON 图层并保存为 `ver_9ecbee0c159bb55b0272eb224f7039a1`。顺序/样式字段的保存由 `project_roundtrip_keeps_layers_style_view_and_source_reference` 覆盖；本次未逐项手工拖动顺序或改样式。）
- [x] 重新打开 V1，确认内容未变化。 — **PASS**（V1 仍为 1166 bytes、SHA-256 `d135642d...`、0 图层；V2 保存后未变化。）
- [x] 重新打开 V2，确认修改存在。 — **PASS**（V2 为 2027 bytes、1 个 `points.geojson` 图层；`prepared` 返回 `available`。）
- [x] 同名保存或并发请求不能覆盖已有版本。 — **PASS**（无覆盖发布测试 `publishing_an_existing_version_id_is_refused...`；临时文件发布测试 `publishing_leaves_no_temporary_links`。未做真实多进程并发压力测试。）
- [x] 模拟写入中断，不应留下被列为有效版本的半成品。 — **PASS**（`interrupted_temporary_files_are_not_listed_and_are_cleaned`：`.tmp-*` 被清理，半成品不进入版本列表。）

## B. 数据源持久化与扫描

- [x] 添加两个文件夹，重启后两者仍存在。 — **PASS**（`folders_persist_and_rescan_sees_added_and_deleted_files`。）
- [x] 向已有文件夹加入一个受支持的新文件，重新扫描后可浏览。 — **PASS**（同一测试添加 `four.fgb` 后可浏览；最终控制台浏览 `points.geojson` 也通过。）
- [x] 新文件不会自动添加到已打开工程。 — **PASS**（最终控制台必须显式点击 **Add layer**；浏览动作不修改工程。）
- [x] 删除源文件后重新打开工程，相关图层不可用且不渲染。 — **PASS**（重启前 E2E：删除 `points.geojson` 后打开，availability 为 `unavailable`，派生工程将该层 `visible=false`。最终二进制另用路径引用 `missing.geojson` 的工程 `prj_b828fe7c5e78c4bc9c99bebb7aeaeb84` 复测，报告 `the file is missing`。）
- [x] 恢复源文件后重新验证，图层可重新加载（若格式/provider 支持）。 — **PASS**（重启前 E2E：恢复文件后 Recheck 为 `available`；工作副本恢复后保存的版本包含 3 个可见图层。最终二进制在 `prj_b828...` 恢复文件后 Recheck 为 `available`；此前在缺失状态下保存的工作副本/版本仍存储 `visible=true`，未把派生隐藏状态写死。另在浏览器中提取 `console.js` 的实际函数做边界验证：文件仍缺失时执行有脏改动的 Recheck，保存时仍恢复 `visible=true` 并移除 `x_rustgis_availability`；文件恢复后保存同样为 `visible=true`。）
- [ ] 无权限目录显示明确错误而非崩溃。 — **NOT RUN**（未构造 chmod/ACL 拒绝目录的实测；代码路径返回 `source_unavailable`/`invalid_input`，但未据此冒充 PASS。）
- [x] `../` 路径穿越被拒绝。 — **PASS**（`traversal_and_absolute_paths_are_rejected`，包含 `../`、`..\\`、绝对路径和混合穿越。）
- [x] 通过符号链接/junction 指向根目录外的路径时按安全策略拒绝或明确限制。 — **PASS**（Unix `symlinks_that_escape_the_root_are_not_served` 通过；Windows junction 未在原生 Windows 实测。）
- [x] 大目录扫描分页/惰性加载，不一次返回所有文件内容。 — **PASS**（`pagination_is_bounded_and_does_not_expose_unsupported_files`；扫描上限 200,000 条、浏览上限 20,000 条。未做超大目录性能测量。）

## C. 栅格与矢量

- [x] 打开小型 GeoTIFF。 — **PASS**（`/tmp/rustgis-final/data/tiny.tif`，486 bytes，16×16，Float32，Deflate，地理范围约 `[-1,1]` lon/lat；由 `cargo run -q -p rustgis-data --example make_preview_geotiff -- /tmp/rustgis-final/data/tiny.tif` 生成。最终控制台从 Sources 显式 **Add layer**，地图以灰度方块渲染，缩放级别约 1:1,850,023；保存为工程 `prj_366bcdb9f500ee0862d8927f08006f89` 的 V2 `ver_d3124cc48aaabac5a812a93585acd959`（2,782 bytes，V1 `ver_3e71c69a5dab36ca7da0fbe84b78b5b4` 仍为 1,167 bytes）；只读重开 V2 后栅格仍渲染，availability 为 `available`，原因 `the raster is embedded in the project`。）
- [ ] 打开 DEM/ASCII Grid（如当前 provider 支持）。 — **NOT RUN**（当前 WSL 无 GDAL，未生成 DEM/ASCII Grid 实测数据。）
- [x] 打开 GeoJSON。 — **PASS**（最终控制台通过 `/api/files/.../points.geojson` 添加，地图显示点图层，availability 为 `available`。）
- [ ] 打开 Shapefile（含必要 sidecar 文件）或 GeoPackage。 — **NOT RUN**。
- [x] 验证 CRS/extent 和基本渲染。 — **PASS**（GeoJSON 点坐标 `[0,0]` 的图层成功渲染并缩放；GeoTIFF 的 16×16 像素也按地理范围渲染。CRS/extent 的独立数值核对未做，故该条只按基本渲染判定。）
- [ ] 使用真实的大型 TIFF（建议 10 GB 测试文件）记录内存峰值、首屏时间和缩放体验。 — **NOT RUN**（无测试数据；不声称性能达标。）
- [ ] 验证非 COG TIFF 不会被一次性读成同等大小的内存缓冲区。 — **FAIL / 已知限制**（实测 486-byte 非 COG TIFF 经 `ui.addUrl` 整文件下载并以 embedded bytes 写入工程；1,377,006-byte COG 则保存为 `maps[0].tiles` 的 URL 流式图层。源浏览器已加 64 MiB TIFF 上限并在超限时拒绝（用 65.0 MiB 的 `large.tif` 实测：toast 为 `large.tif is 65.0 MiB...`，工程保持 `saved`，没有发起图层导入），避免通过该入口误下载大非 COG TIFF；这不是对 provider 大文件路径的 PASS，直接使用地图的 Add Data From URL 仍可能整文件下载非 COG。未做 10 GB 实测。）
- [x] 如果实现 Range，验证 206 Partial Content、Content-Range、Content-Length 与 seek/range 读取。 — **PASS**（最终服务：`Range: bytes=0-9` 返回 `206`、`Content-Range: bytes 0-9/135`、`Content-Length: 10`、body 10 bytes；实现使用 `File::take` 流式读取。）
- [ ] 删除缓存后重新打开工程，缓存能重建且源数据不受影响。 — **NOT RUN**（未单独验证缓存删除/重建路径。）
- [x] COG 流式图层的缺失/恢复和保存可见性。 — **PASS**（`tiny_cog.tif` 由 `cargo run -q -p rustgis-data --example make_preview_cog -- /tmp/rustgis-final/data/tiny_cog.tif` 生成。工程 `prj_1a228892acab13f57679fb21cd9654f4` 的 V2 `ver_3bcc832a9936fde88ac606e675dfd811` 将 1,377,006-byte 文件保存为 `maps[0].tiles[0]`，URL 为 `http://127.0.0.1:8765/api/files/src_5951172671638b1b2a203dd16f42f587/tiny_cog.tif`，kind 为 `cog`。删除源文件后 Recheck 返回 `unavailable` / `the file is missing`，地图中该 tile 图层被隐藏；恢复文件后 Recheck 返回 `available`，图层重新可见。工作副本 `wrk_28864ba4dc00a475d580ddf1f863a3bf` 在文件缺失且 tile 被隐藏时保存为 V3 `ver_17af796d8ab6230d0cd1e41523101720`，落盘内容仍为 `visible: true`、无 `x_rustgis_availability` 标记，V2 未被改写；恢复文件后 V3 的 prepared 视图再次为 `available`。远程 COG URL 仍按 `unknown` 处理，不做网络探测。）

## D. PostgreSQL/PostGIS

- [x] 添加连接并浏览可访问的数据库。 — **PASS**（真实 PostGIS 容器 `rustgis-pg-test`，端口 55432；`/databases` 列出数据库，连接配置持久化。）
- [x] 浏览 schema、表、视图和空间列。 — **PASS**（`/catalog` 返回 `public`、`tiger`、`tiger_data`、`topology`；`public.preview_points`、`public.preview_plain` 等关系。未单独创建 view 做专项测试，catalog 查询包含 view/materialized view/foreign table。）
- [x] 把一个 PostGIS 表作为图层加入地图。 — **PASS**（重启前 E2E：`preview_points` 120 行，EPSG:4326、limit=5 快照，`ui.addUrl` 成功加入图层。）
- [x] 重启后连接配置仍在；密码按安全设计恢复或要求重输。 — **PASS**（重启后 `/catalog` 返回 401 `password_required`；重新 POST 密码后 catalog 恢复；`databases.json` 只有 `password_required`，没有密码。）
- [x] 数据库断开时显示不可用，不崩溃。 — **PASS**（停止 `rustgis-pg-test` 后 catalog 返回 503 `database unavailable: ... connection timed out`；容器恢复后 catalog 再次返回 200。）
- [ ] 删除表或重命名 schema 后，工程图层显示不可用。 — **NOT RUN**（未对真实工程图层执行删表/改 schema；不冒充已验证。）
- [ ] 无权限对象不会泄漏额外信息或导致崩溃。 — **NOT RUN**（未构造受限数据库账号专项测试。）
- [x] 确认数据库密码不在项目 JSON、日志、URL 或 API 响应中。 — **PASS**（`databases.json` 和测试日志中未出现测试密码；snapshot URL 不含凭据；API 视图只返回 `password_required`；错误文本经 URL userinfo 脱敏。）

## E. 部署与恢复

- [ ] Windows 上启动并访问。 — **NOT RUN**（当前 Windows 环境没有 `cargo`/`rustc`；未做 Windows 原生构建或运行。）
- [x] Linux 上启动并访问。 — **PASS**（WSL2 Ubuntu 上运行最终二进制；非 WSL 的独立 Linux 宿主未单独实测。）
- [x] WSL 中启动，并从宿主浏览器访问。 — **PASS**（Windows 浏览器访问 `http://127.0.0.1:8765/`，`/api/health` 返回 `web_ready:true`，控制台和地图加载正常，浏览器 console 无错误。）
- [x] Docker 容器重建后，挂载卷中的数据源配置和工程版本仍存在。 — **PASS**（最新镜像冒烟：命名卷 `rustgis-final-smoke3-data` 中创建源 `src_feb8394c247e13998a90cd7593e6bc1e`（`/srv/gis`）和工程 `prj_bacbc975eb03a40866d2798f4dcb402b` / V1 `ver_b48674d02abaff992e6a175c61db505e`，`docker rm -f` 后以同一卷重建容器，`/api/sources/folders` 和 `/api/projects/{id}` 均恢复，版本数仍为 1。此前旧镜像的 `rustgis-final-smoke2-data` 冒烟也通过。）
- [x] 原始数据目录可独立只读挂载；持久化目录可写。 — **PASS**（容器将宿主临时目录以 `:ro` 挂载到 `/srv/gis`；容器内 `touch /srv/gis/nope` 返回 `Read-only file system`，`touch /data/writable` 成功。容器以 UID 10001 运行，健康检查为 `healthy`；`/api/files/.../points.geojson` 的 `Range: bytes=0-9` 返回 206、`Content-Range: bytes 0-9/144`（原生最终服务的同一请求还返回 `Content-Length: 10`）。最终镜像也验证了 `/console.js` 含最新可见性恢复逻辑。）
- [ ] 使用相对/绝对路径、Windows 盘符和 Linux 挂载路径分别验证。 — **NOT RUN**（Linux 相对/绝对路径有单元测试覆盖；Windows 盘符路径和跨 Windows/Linux 路径格式未实测。）
- [x] 服务只绑定预期地址；文档提醒不要未经防护暴露公网。 — **PASS**（原生默认 `127.0.0.1:8765`；Docker 发布端口为 `127.0.0.1:8765`；`docs/preview.md` 明确说明不是认证边界。）

## F. 并发与稳定性

- [ ] 3 个浏览器会话同时浏览不同图层。 — **NOT RUN**（仅使用一个控制台会话做功能验证。）
- [ ] 10 个会话的基本预览压力测试。 — **NOT RUN**。
- [ ] 20 个会话作为目标压力测试；记录 CPU、内存、响应时间和错误率。 — **NOT RUN**。
- [ ] 同一数据源重复打开不会触发无界的重复预处理。 — **NOT RUN**（没有做重复打开/内存增长测量。）
- [ ] 长时间扫描或数据库断开不会冻结地图界面。 — **NOT RUN**（数据库断开返回 503 的快速验证已通过；长时间扫描/地图交互未测。）
- [x] 所有失败响应均有明确错误，不输出秘密信息。 — **PASS**（已检查 405 文件写入、路径穿越、缺失文件、密码缺失 401、数据库断开 503、非法表名/无几何列错误；均返回结构化错误。该结论覆盖已测失败路径，不是对全部输入空间的穷举。）

## 版本发布门槛

当前已实现阶段的窄测试、完整串行 CI、最终控制台 E2E、Range 和 Docker 持久化检查通过。以下项目仍未运行，不能据此宣称完整产品验收通过：

- 10 GB TIFF/COG、非 COG 远程窗口读取及大栅格性能；非 COG TIFF 仍会被整文件下载（源浏览器 64 MiB 上限只是入口保护）；
- Windows 原生运行、非 WSL Linux 宿主、Windows 盘符与跨平台路径；
- 删除表/改 schema、无权限数据库对象；
- 3/10/20 会话并发压力、长时间扫描/数据库断开的交互稳定性。

在这些项目完成实测前，发布说明必须保留相应的 **NOT RUN** 标记，不得用推测代替结果。

## 2026-10-24 交互模型重构(草稿优先 + 工程元数据)

按用户 7 项需求重构,浏览器自动化实测(数据目录 `/home/ubuntu/rustgis-preview/home`,旧测试工程已按用户要求清除,数据源配置保留):

1. 打开页面默认无工程、草稿模式,可直接添加图层;`beforeunload` 在草稿有内容或工作副本有未保存修改时提醒。 — **PASS**(草稿添加 `points.geojson` → "有未保存的内容";监听器注册已验证)
2. 草稿经「保存为工程…」弹窗填写名称/描述后创建 V1 并继续编辑。 — **PASS**(工程「城市监测底图」创建成功,描述落库)
3. 打开版本默认只读。 — **PASS**("只读版本"徽标,保存按钮禁用)
4. 工作副本可编辑、可保存。 — **PASS**
5. 复制/保存新版本均弹窗填写标题,不再以裸 ID 展示。 — **PASS**(「野外调绘副本」「调绘成果版」在卡片中正确显示)
6. 卡片按工程创建时间排序、分组显示工程(名称/描述/创建·更新时间)与版本(V 编号+标题+时间)、工作副本(标题+基于版本)。 — **PASS**(服务端 `projects()` 改为 created_at 排序,`projects_are_listed_by_creation_time` 测试)
7. 工程信息(名称/备注)可编辑,创建时间不可变。 — **PASS**(PATCH 后 created_at 不变、updated_at 前移;`project_metadata_is_editable_with_titles_and_survives_restart` 与 HTTP 层 `project_metadata_is_editable_over_the_api` 测试)

服务端新增:`ProjectMeta.description`、`VersionMeta/WorkingCopyMeta.title`(serde default,旧索引兼容)、`PATCH /api/projects/{id}`、创建/复制/保存接口接受 title 与 description。测试:`cargo test -p rustgis-preview` 38 项全部通过。修复过程中发现并修正弹窗 `modal-title` id 与标题冲突导致提交静默失败的缺陷。已知限制:关闭页面的浏览器原生提醒弹窗在自动化中未逐一手动确认(监听器与状态逻辑已验证);嵌入式地图 WASM 应用界面仍为英文。
