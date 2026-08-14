# DSH Desktop

DeepSeek Harness 的桌面端打包项目 —— 基于 [Tauri 2](https://v2.tauri.app/) 的轻量桌面壳。

> **本仓库仅负责桌面打包**：启动时自动拉起官方 `@deepseek-ai/dsh` 包的 web 服务，
> 然后把界面加载到原生窗口。仓库内**不包含** DeepSeek Harness 的任何业务源码，
> 也不是 DeepSeek Harness 的分支或重新实现。
>
> ⚠️ 非官方项目，与 DeepSeek 官方无关。运行时通过 `npx` 从 npm 获取官方包
> [`@deepseek-ai/dsh`](https://www.npmjs.com/package/@deepseek-ai/dsh)。

## 功能特性

- 🚀 **一键启动**：应用启动即拉起 dsh web 服务（`http://127.0.0.1:3080`），就绪后自动加载界面
- 🚫 **无黑窗**：Windows 下使用 `CREATE_NO_WINDOW` 启动子进程，不会弹出控制台黑窗
- 📌 **系统托盘**：点击关闭按钮仅隐藏到托盘，应用继续在后台运行；托盘左键恢复窗口，右键菜单提供「开机自启 / 显示主窗口 / 退出」
- 🔂 **单实例**：重复启动不会开新进程，而是唤起已有实例的主窗口（即使藏在托盘也会拉出）
- ⚡ **开机自启**：托盘菜单一键开关，随 Windows 登录自动启动
- 🛡️ **干净回收**：从托盘退出时通过 `taskkill /T` 回收整棵服务进程树
- 🎨 **官方鲸鱼标志**：应用图标取自 deepseek-harness 的鲸鱼标志，生成全套 ico / icns / png 图标

## Tauri 插件

| 插件 | 用途 |
| --- | --- |
| [tauri-plugin-opener](https://v2.tauri.app/plugin/opener/) | 用系统默认应用打开外部链接 |
| [tauri-plugin-single-instance](https://v2.tauri.app/plugin/single-instance/) | 单实例运行，二次启动唤起主窗口 |
| [tauri-plugin-autostart](https://v2.tauri.app/plugin/autostart/) | 开机自启（托盘菜单勾选开关，Windows 写入 HKCU Run 键） |

> 说明：以上插件均从 Rust 侧调用，无需给前端配置 capabilities 权限。
>
> 后续可选集成：`updater`（自动更新，需签名公钥 + 托管更新源）、
> `notification`（"已最小化到托盘"提示，Windows 下需安装后经 NSIS 快捷方式注册 AUMID 才会弹出）、
> `window-state`（记忆窗口大小/位置）、`log`（日志落盘）。


## 工作原理

```
启动 dsh-desktop
  └─ spawn: npx --yes @deepseek-ai/dsh web   （无控制台窗口）
       └─ 轮询 127.0.0.1:3080 直至服务就绪（最长 60s）
            └─ 窗口 navigate 到服务地址，加载 DSH Web 界面

关闭窗口  → 仅隐藏到托盘，服务继续运行
托盘「退出」→ 结束子进程树并退出应用
```

## 环境要求

| 环境 | 说明 |
| --- | --- |
| Windows 10 / 11 | 依赖 WebView2（Win11 自带，Win10 一般已随更新安装） |
| Node.js ≥ 18 | 运行时通过 `npx` 拉取 `@deepseek-ai/dsh` |
| Rust（仅构建需要） | stable 工具链，<https://rustup.rs> |
| pnpm（仅构建需要） | <https://pnpm.io> |

## 开发

```sh
pnpm install
pnpm tauri dev
```

> 开发模式下主进程会保留控制台窗口以便查看日志；打包后的应用无任何控制台窗口。

## 构建安装包（NSIS）

```sh
pnpm tauri build
```

产物位于 `src-tauri/target/release/bundle/nsis/dsh-desktop_<version>_x64-setup.exe`，
安装后桌面与开始菜单均使用仓库内生成的鲸鱼图标。

## 项目结构

```
.
├── src/                  # 前端壳页面（启动加载页）
│   ├── index.html        # 加载页：品牌图标 + 启动状态
│   └── icon.png          # 加载页图标（与 icons/icon.png 同步）
└── src-tauri/
    ├── src/
    │   ├── main.rs       # 入口（release 下无控制台子系统）
    │   └── lib.rs        # 服务拉起/回收、托盘、关闭进托盘、单例唤起、开机自启逻辑
    ├── icons/            # 由 tauri icon 生成的全套应用图标（含 tray-icon.png）
    ├── capabilities/     # Tauri 权限清单
    └── tauri.conf.json   # 窗口、打包（NSIS）、图标配置
```

## 图标

图标源取自 deepseek-harness 的 `website/public/favicon.svg`（鲸鱼标志），
由 `tauri icon` 从合成 SVG 生成全套尺寸：

- 桌面 / 任务栏 / 安装包：`src-tauri/icons/icon.ico`（含多尺寸）
- 系统托盘：`src-tauri/icons/tray-icon.png`
- 加载页：`src/icon.png`

## License

[MIT](./LICENSE)
