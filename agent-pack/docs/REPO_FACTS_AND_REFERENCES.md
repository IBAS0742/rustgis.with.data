# RustGIS 事实、参考与源码核验事项

## 公开文档中可确认的事实

以下结论基于 RustGIS 官方公开文档。网页会更新，开发时应以锁定的 Git commit 和实际源码为准。

- RustGIS 是分层 Rust workspace，包含几何、数据、地图、分析、瓦片、渲染、引擎、UI 和应用层。
- RustGIS Web 在浏览器中运行；浏览器的文件选择和下载行为与桌面版不同。
- 官方数据文档列出 GeoJSON、CSV、Shapefile、GeoPackage、GeoParquet、FlatGeobuf、KML/GPX、GeoTIFF/COG、ASCII Grid 等格式，具体支持范围按当前版本文档核实。
- PostgreSQL/PostGIS 直连明确列在桌面端能力中；不能假定 Web 前端可直接访问服务端本地目录或数据库。
- `.rgis` 是 RustGIS 工程格式；工程内容可能对不同数据类型采用嵌入或外部引用，必须通过源码和实测确认。
- 官方仓库提到 `cargo xtask ci` 作为 CI/质量检查入口，但需先确认锁定版本存在且命令可用。

## 官方链接

- RustGIS 首页：https://rustgis.app/
- 安装与 Web app 说明：https://rustgis.app/installation/
- 数据说明：https://rustgis.app/user-guide/data/
- 支持格式：https://rustgis.app/user-guide/data-formats/
- 架构说明：https://rustgis.app/architecture/
- GitHub 仓库：https://github.com/opengeos/RustGIS
- 仓库 Agent 指令：https://github.com/opengeos/RustGIS/blob/main/AGENTS.md

## 必须从实际源码确认的事项

1. 当前 checkout 的 commit 和工作区是否干净。
2. Web app 的真实 crate、前端入口、静态资源构建和启动命令。
3. `.rgis` 文件的真实 schema、序列化器、兼容策略和项目加载流程。
4. 保存命令是否自动写回文件，是否存在自动保存或偏好设置副作用。
5. 图层的 source URI/路径保存方式、数据源恢复时的错误路径。
6. 栅格读取是否支持窗口读取、内部 overview、HTTP Range 或其他按需策略。
7. 数据 provider trait/接口能否被服务器端复用。
8. PostGIS provider 使用的驱动、feature iteration、空间过滤和凭据处理方式。
9. RustGIS 是否已有目录浏览、sources registry 或项目 package 的可复用命令。
10. CI、格式化、Clippy、测试和 layering checks 的实际命令。

## 防止误判

- 桌面版支持不等于 Web 版支持。
- 支持 `.rgis` Save/Save As 不等于具备不可变版本管理。
- 支持 PostGIS provider 不等于可通过浏览器访问服务端 PostgreSQL。
- “可以打开 GeoTIFF”不等于 10 GB 非 COG 文件一定能即时交互浏览。
- 编译成功不等于工程缺失图层、跨平台路径和版本不可覆盖均正确。
