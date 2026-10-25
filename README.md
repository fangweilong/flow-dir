# FlowDir

Windows 文件管理器：多窗格 + 标签、原生 Shell 集成、低内存占用。Tauri 2 + Rust 后端，React 19 前端。

[![release](https://github.com/fangweilong/flow-dir/actions/workflows/release.yml/badge.svg)](https://github.com/fangweilong/flow-dir/actions/workflows/release.yml)

## 特性

- **布局**：单窗格、左右/上下双窗格、三窗格、四窗格；每窗格无限标签，标签可跨窗格拖动，拖到窗格边缘即拆分。
- **浏览**：批量枚举 + 虚拟列表（2.5 万项时 DOM 仅几十行）、面包屑与前进/后退、`Ctrl+L` 地址栏、磁盘下拉切换、文件名筛选、隐藏/系统项开关。
- **搜索**：优先使用 Everything（本机运行时），不可用时自动降级为内置 `jwalk` 扫描，`Ctrl+F` 呼出。
- **预览**：WIC 缩放解码图片、PDF 首页、视频首帧、文本（BOM / UTF-8 / GBK 回退）、ZIP 目录、EXE/DLL 版本信息，超过 64 MB 不生成预览。
- **文件操作**：复制、移动、回收站删除、重命名、新建文件夹，全部走 `IFileOperation`，带进度与取消；文件/文件夹右键为系统原生菜单（含第三方扩展）。
- **系统集成**：单实例、托盘、原生窗口阴影与深色菜单、`Alt+Enter` 属性页。
- **外观与语言**：深浅色/跟随系统、中英双语。
- **资源占用**：WebView2 GPU 加速可在设置中关闭以省内存，最小化自动挂起渲染进程。
- **自动更新**：按可配置间隔检查 GitHub Release，单文件版原地替换，安装版静默升级。

## 截图

| 主界面 | 多窗格 |
| --- | --- |
| ![主界面](screenshots/home.png) | ![多窗格](screenshots/panes.png) |

| 预览 | 搜索 |
| --- | --- |
| ![预览](screenshots/preview-image.png) | ![搜索](screenshots/search-everything.png) |

## 安装

从 [Releases](https://github.com/fangweilong/flow-dir/releases) 下载，二选一：

| 文件 | 说明 |
| --- | --- |
| `FlowDir_<version>_x64_portable.exe` | 单文件版，免安装，更新时原地替换自身 |
| `FlowDir_<version>_x64-setup.exe` | NSIS 安装版，安装到当前用户目录，写入卸载项，更新时静默升级 |

要求 Windows 10/11 x64，并具备 WebView2 运行时（Windows 11 及较新的 Windows 10 已内置）。两种形态共用同一份配置目录。

## 更新

设置 → 更新：显示当前版本与构建形态，可手动检查、一键下载安装；自动检查间隔可选 6/12/24/72 小时或关闭（默认 24 小时，`0` = 仅手动）。

版本信息来自 Release 资产 `latest.json`（版本号 + 两个安装包的文件名、大小、SHA-256），应用下载后会校验大小与前 2 字节 PE 头、SHA-256 摘要，通过后才执行替换/安装。仓库需保持 **public**，否则匿名拉取 `latest.json` 会失败。

## 开发

依赖：Node.js 22+、Rust stable（MSVC，见 `src-tauri/rust-toolchain.toml`）、Visual Studio Build Tools 与 Windows SDK。

```bash
npm ci
npm run tauri dev          # 开发模式（Vite + Tauri）
npm run tauri build        # 产物：src-tauri/target/release/flowdir.exe + NSIS 安装包
```

Rust 侧单测：

```bash
cd src-tauri && cargo test
```

## 配置与数据

| 路径 | 内容 |
| --- | --- |
| `%APPDATA%\FlowDir\settings.toml` | 语言、主题、GPU、快速访问、更新间隔等（带注释，手工改也安全） |
| `%APPDATA%\FlowDir\layout.json` | 窗格/标签/窗口几何，启动恢复；损坏时降级并保留 `.rejected.json` |
| `%APPDATA%\FlowDir\update.json` | 上次更新检查时间与最近一次发现的版本 |
| `%LOCALAPPDATA%\FlowDir\EBWebView` | WebView2 浏览器配置 |

## 发布

版本号在 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 三处必须一致。发版只需同时递增三处并 push `main`：`release.yml` 检测到 `v<version>` 尚未发布会自动编译、打包并创建 Release（单文件版、NSIS 安装包、`latest.json`）。版本未变则整条流水线跳过；需要重建已发布版本时手动触发 workflow 并勾选 `force`。

细节与约束见 [AGENTS.md](AGENTS.md)。

## 目录结构

```
src/              React 前端（文件列表、预览、搜索、设置）
src-tauri/src/    Rust 后端（枚举/扫描、文件操作、预览、Shell 菜单、更新）
src-tauri/icons/  应用图标
screenshots/      README 与文档截图
scripts/          CDP/窗口级验证脚本
.github/          Release 流水线
```

## 已知限制

- 仅支持 Windows。
- 从列表拖出到资源管理器未实现（Windows 拖拽源受限），复制/剪切走剪贴板。
- Everything 为可选依赖，未安装或未运行时使用内置扫描。
