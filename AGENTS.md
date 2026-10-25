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
