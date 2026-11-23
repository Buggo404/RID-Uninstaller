<div align="center">

<img src="client/crates/rid-app/app-icon.png" width="112" alt="RID" />

# RID Uninstaller

**把程序卸干净，而且删错了能找回来。**

Windows 上的卸载工具：卸载程序、清理残留、管理启动项。用 Rust 编写，界面基于 Tauri。

[![License](https://img.shields.io/badge/license-MIT-black)](LICENSE)
![Platform](https://img.shields.io/badge/Windows-10%20%2F%2011%20x64-black)
![Rust](https://img.shields.io/badge/Rust-2024-black)
![Tauri](https://img.shields.io/badge/Tauri-2-black)

[Bug 反馈](https://github.com/Buggo404/RID-Uninstaller/issues) · [English](#english)

</div>

---

## 它解决什么问题

系统自带的“卸载”只是运行程序自己的卸载器，留下的文件夹、注册表项、服务和快捷方式没人管。而很多清理工具的问题正好相反：按名字一通匹配，把别的程序甚至你的文档一起删了。

RID 的做法是**每一项残留都要说得出理由，每一次删除都能撤销**。

## 功能

| | |
| --- | --- |
| **卸载** | 注册表登记的程序、MSI、Microsoft Store 应用，以及 Steam、Scoop、Chocolatey 安装的内容，都在一个列表里 |
| **残留扫描** | 卸载后查找剩下的文件夹、注册表项、服务、快捷方式；每一项都标明依据，只有确定属于该程序的才默认勾选 |
| **强制删除** | 卸载器坏了或者早就不存在的程序，直接按残留扫描的结果清理 |
| **批量与静默** | 一次选多个；卸载器支持时可全程不弹窗 |
| **隔离区** | 删除的内容先移入隔离区，可以整批还原；保留时长可设，也可以选择直接永久删除 |
| **启动项** | 注册表 Run、启动文件夹、计划任务、Store 应用的开机任务，统一查看、禁用、删除、添加 |
| **右键菜单** | 在资源管理器里对快捷方式直接“卸载”，对安装包“安装并追踪” |
| **AI 分析** | 右键任意程序、启动项或残留项

界面支持简体中文和英文，深色、浅色跟随系统。

## 为什么不会误删

这是这个项目最在意的部分。

- **有护栏**：Windows 目录、Program Files 根目录、用户文档等位置，以及其他已安装程序的目录，无论扫描结果怎样都拒绝删除。删除前会对每一项重新检查一遍。
- **有依据**：残留项带着它被找到的原因（安装位置、卸载器所在目录、名称匹配、安装记录……）。只凭名称相似找到的项不会默认勾选。
- **看得见文档**：文件夹里如果有文档类文件，会明确提示数量。
- **能反悔**：默认所有删除都进隔离区，文件原样保留，注册表先导出再删。

## 当前状态

项目处于早期阶段，还没有正式发布版本。每次提交都会由 [GitHub Actions](https://github.com/Buggo404/RID-Uninstaller/actions) 自动编译出安装包（尚未做代码签名），可以在对应的运行记录里下载。安装后程序会自己检查并安装新版本：更新包必须带有与程序内置公钥匹配的签名才会被接受。

| 部分 | 状态 |
| --- | --- |
| 卸载、残留扫描、隔离区、启动项、右键菜单、命令行 | 可用 |
| 规则库与更新服务端、管理后台 | 可用 |
| 安装追踪（记录安装程序创建的每一项，卸载时照单清理） | 代码已完成，**内核驱动尚未取得微软签名**，在普通系统上暂时无法加载 |
| AI 分析 | 可用；需要在服务端后台配置模型（兼容 OpenAI 与 Anthropic 接口），提示词可按功能分别修改 |

## 构建

需要 Windows 10/11 x64，以及：

- Rust（stable，支持 2024 edition）
- Node.js 24 与 pnpm 10
- 只有编译驱动时才需要 Windows Driver Kit 和 LLVM

```powershell
cd client
.\scripts\build.ps1
```

脚本会依次运行测试、构建命令行和程序本体，产物是安装包 `client\target\release\bundle\nsis\Rid Uninstaller_<版本>_x64-setup.exe`。

- 程序默认连接 `https://rid.bitheart.top`；编译时设置环境变量 `RID_SERVER` 可以指向别的服务器（例如本机的测试服务）。
- 加上 `-Driver <已由微软签名的 RidMonitor.sys>` 会把驱动一并打进程序。
- 加上 `-BuildDriver` 会同时编译驱动。

### 隐私



开发界面：

```powershell
cd client\crates\rid-app
pnpm install
pnpm tauri dev
```

### 服务端

规则库、更新检查和管理后台，基于 Node.js、Vue 3 和 PostgreSQL，用 Docker 启动：

```powershell
cd server
Copy-Item .env.example .env   # 填写数据库密码和管理员令牌
docker compose up -d --build
```

管理后台在 `/admin`。初始管理员密码只在首次启动的日志里出现一次，登录后必须修改。

常见程序的残留规则在 `server/seed/rules.txt`，用 `node scripts/seed.mjs` 导入；某个程序对应的键用 `rid key "<程序名称>"` 得到。

## 目录

```
client/
  crates/rid-core     枚举、扫描、安全护栏、删除与还原（不依赖界面）
  crates/rid-app      桌面程序（Tauri + Vue 3）
  crates/rid-cli      命令行
  crates/rid-proto    与驱动共用的协议
  driver/rid-monitor  安装追踪用的文件系统过滤驱动（Rust，no_std）
server/               规则库、更新、管理后台
```

## 反馈

遇到问题请到 [Issues](https://github.com/Buggo404/RID-Uninstaller/issues) 提交。如果是误删或漏删，附上程序名称和相关路径会很有帮助——这类问题优先处理。

## 许可证

[MIT](LICENSE)

## Star 趋势

[![Stargazers over time](https://starchart.cc/Buggo404/RID-Uninstaller.svg?variant=adaptive)](https://starchart.cc/Buggo404/RID-Uninstaller)

---

## English

RID Uninstaller removes Windows programs completely: it runs the program's own uninstaller, then finds what was left behind (folders, registry keys, services, shortcuts) and removes it. Every leftover carries the reason it was found, protected locations and other programs' folders are refused no matter what a scan says, and everything removed goes to a quarantine it can be restored from.

It also manages startup entries, lists Store apps and packages from Steam, Scoop and Chocolatey, and ships a command-line tool. Written in Rust with a Tauri interface; English and Simplified Chinese.

Status: early (0.1.0), no releases yet — build it with `client\scripts\build.ps1`. Install tracing depends on a kernel driver that is not yet signed by Microsoft.
