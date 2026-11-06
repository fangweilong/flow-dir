# FlowDir 工程规则

## 发版与自动更新

- 版本号只存在三处：`package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`。发版必须同时递增且完全一致；`release.yml` 校验不一致会直接失败。
- 不要手动打 tag、也不需要手动跑构建：改完三处 version 后 push 到 `main`，`.github/workflows/release.yml` 会检测 `v<version>` 不存在，自动编译并发布 Release（tag 由 action 创建）。只有改动这三个版本文件才会触发；版本已发布时重复推送会跳过。
- 每次 Release 必须同时包含 `FlowDir_<ver>_x64_portable.exe`（单文件）、`FlowDir_<ver>_x64-setup.exe`（NSIS）和 `latest.json`（版本 + 两资产 name/size/sha256）。缺一个对应更新路径即失效；不要手改资产名或手写 manifest。
- 仓库必须保持 public：应用匿名访问 `https://github.com/<repo>/releases/latest/download/latest.json` 与同名资产；私有仓库需要先给更新器加令牌认证。
- `FLOWDIR_UPDATE_REPO` 由 workflow 在编译期注入（`option_env!`）。本地构建未设置时更新检查返回 `UPDATE_SOURCE`；可用同名环境变量在运行时临时覆盖。
- 需要重建已存在的版本时，用 `workflow_dispatch` 并勾选 `force`，不要删除 tag 重发。
- 自动检查间隔存 `settings.toml` 的 `update_interval_hours`（默认 24，`0` = 仅设置页手动检查），实现见 `src-tauri/src/update.rs`（WinHTTP + CNG，无额外网络/加密依赖）。
- 升级方式按构建形态自动分流：单文件版改名自身为 `*.old.exe` 后原地替换并等待进程退出再启动；安装版（同目录有 `uninstall.exe`）以 `/S` 静默执行 setup。修改这两条路径前先读该文件的注释与测试。
- 开发版与正式版靠 identifier 区分：日常开发必须用 `npm run dev:app`（叠加 `src-tauri/tauri.dev.conf.json`，identifier `dev.flowdir.app.dev`）。单实例互斥名、`%APPDATA%\FlowDir-dev`、`%LOCALAPPDATA%\FlowDir-dev`、标题 `FlowDir Dev` 均由该后缀派生（见 `src-tauri/src/store.rs`），不要另建第二套开关。`release.yml` 的正式构建不带 `--config`，不要为此修改工作流。

## 文件保护（危险操作拦截）

- 删除、移动、重命名的路径保护统一在 `src-tauri/src/protect.rs`，由 `ops::validate()` 在 GUI 与提权 helper 两条路径执行；新增操作入口必须走这里，不要只在前端某处检查。
- 规则来自 `settings.toml` 的 `protect_block`（硬拒绝，返回 `PROTECTED`）与 `protect_confirm`（要求输入确认，返回 `NEEDS_CONFIRM`）；驱动器根目录与 UNC 共享根目录始终硬拒绝。
- 匹配只覆盖规则目录本身及其上级目录，子目录不受影响；`protect_confirm` 的确认由前端输入指定文字后通过 `confirmedProtected` 回传，后端重新校验，不能靠前端跳过。
- 默认块规则使用 `%SystemRoot%` 等环境变量，`Settings::normalize()` 会把它们展开成绝对路径再保存。

## WebDAV

- 虚拟地址统一为 `webdav://<connection-id>/<相对路径>`。`id` 是 `settings.toml` 中 `webdav[].id` 的稳定键，也用于凭据管理器；显示名随时可改，禁止把显示名写进标签页、历史或布局。前端注册表（`src/paths.ts#setWebdavConnections`）只负责面包屑与标签标题。
- 密码只存 Windows 凭据管理器（`src-tauri/src/credentials.rs`，target `FlowDir/WebDAV/<id>`）。密码不得出现在 `settings.toml`、日志或任何 IPC 响应里；`webdav_set_password` 不依赖设置缓存，新增连接可先写凭据再等防抖保存。
- 协议实现集中在 `src-tauri/src/webdav.rs`（WinHTTP + `quick-xml`，不新增网络/加密依赖）。TLS 忽略仅按连接生效，不得改成全局开关。
- 任何涉及 `webdav://` 源或目标的复制/移动/删除/重命名/新建都必须走 `ops::validate`，再由 `ops::run` 的 `involves_webdav` 路由到 `webdav_ops::run`；不要绕过保护、进度与取消另开通道。远端没有回收站，前端删除必须强制 `permanent: true`，且不记录可恢复的撤销项。
- 内部剪贴板：`clipboard.rs` 只把本地子集写入 `CF_HDROP`，完整选择留在进程内并以剪贴板序列号失效；不要假设系统剪贴板能表达 `webdav://`。
- 搜索、目录大小、去重、压缩/解压、哈希、终端、属性页只支持本地路径，菜单必须隐藏；`list_dir`、`tree_dirs`、`path_is_dir`、`paths_exist`、`unique_paths`、`open_path`、`preview_*` 已按路径分流，新增命令时保持同样的判断。

## 前端架构与模块划分规范

- **禁止在 `App.tsx` 堆积业务与弹窗**：`App.tsx` 仅作为顶层纯编排层（工具栏骨架、窗格容器挂载与全局快捷键分发）。单个组件或 Hook 文件代码行数原则上不超过 800 行，严禁将多模块逻辑重新塞回单一文件。
- **业务状态机强制 Hook 隔离**：
  - 文件底层 I/O、进度管理、撤销/重做栈与提权恢复统一维护在 `src/hooks/useFileOperations.ts`；
  - 跨窗格拖放、系统 OLE 拖出与 WebView2 原生拖入及放置目标检测统一维护在 `src/hooks/useDragAndDrop.ts`；
  - 侧边栏目录树分级缓存、多路径失效广播与 Git 状态异步合并统一维护在 `src/hooks/useTreeAndGit.ts`；
  - 新增跨模块业务状态优先按领域收敛至 `src/hooks/`，严禁在顶层组件中直接暴露底层实现。
- **弹窗与浮层统一收敛**：所有低频模态窗口、右键菜单及浮层统一由 `src/ModalContainer.tsx` 编排挂载，严禁在 `App.tsx` 顶层散落编写弹窗 JSX。

## 打包分包与体积红线（Code Splitting）

- **低频模态组件强制按需加载**：设置面板、文件比较、多重命名、校验/去重、解压/压缩等非首屏必须的模态组件，必须通过 `React.lazy` 动态导入并配合 `<Suspense fallback={null}>` 挂载，严禁在应用主链路静态同步 `import`。
- **第三方依赖隔离打包**：`vite.config.ts` 必须保持 `manualChunks` 配置，稳定第三方库（`react` / `react-dom`、`@tanstack/react-virtual`、`@tauri-apps` 等）独立分包，避免随业务代码频繁失效缓存。
- **体积与警告红线**：
  - 生产构建产物（`npm run build`）中**严禁出现单个 Chunk 超过 500 kB** 的 Rollup 告警；
  - 首屏核心主 Chunk（`index-*.js`）体积严格限制在 **300 kB 以下**；
  - 每次前端代码修改必须通过 `npx tsc --noEmit`（0 错误）与 `npm run build`（0 大体积告警）双重验收。
