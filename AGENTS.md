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
