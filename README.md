[English](README.en.md) · [文档](docs/) · [下载](https://github.com/larkswitch/larkswitch/releases/latest)

# larkswitch

**同一个飞书 App 下挂多个账号，每条命令单独指定用谁执行，不用退出重登。**

[![Release](https://img.shields.io/github/v/release/larkswitch/larkswitch?include_prereleases&label=release)](https://github.com/larkswitch/larkswitch/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-informational.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-informational)](https://github.com/larkswitch/larkswitch/releases/latest)

<sub>非官方第三方工具，与字节跳动 / 飞书 / Lark 无任何关联。</sub>

<!-- 发布前替换为 15-20 秒真机录屏 GIF（录制脚本见 docs/assets/demo.tape），
     640-800px 宽、<3MB。GIF 就位后删掉这行注释和下面的 switch.svg。 -->
![托盘换人只影响下一条命令，正在跑的命令保持启动时的身份](docs/assets/switch.svg)

<sub>看点：托盘点了「李四」之后，**新起的命令**是李四，**正在跑的命令**仍然是张三。</sub>

```bash
# 一条命令一个身份，全局 active 不动
lark-cli --account alias:bot whoami                      # 机器人号
lark-cli --account alias:me  whoami                      # 本人号
lark-cli --account alias:me  calendar +agenda --as user
```

## 这是什么

官方 `lark-cli --profile` 管的是 **App 配置**。v1.0.5 起可以管多个 App，但**同一个 App ID 下永远只有一套身份**。

写飞书应用的人几乎都会撞到这堵墙：自测至少要两个账号——机器人一个、本人一个；做外包和顾问的还要在几个客户租户之间来回跳。现在的解法是退出、重登、再退出、再重登。

larkswitch 用官方支持的 `LARKSUITE_CLI_CONFIG_DIR`，把每个 `(App, User)` 隔离成独立配置目录，并决定下一条命令用谁：

- **一个 App，多个人** —— 每个账号一个隔离配置目录，互不污染。
- **命令级身份** —— `--account` 只影响这一条命令，不动全局 active。
- **正在跑的命令不被换脸** —— 命令启动时冻结身份快照。并发跑多个身份不会串。
- **Token 不经手** —— 不保存 Access / Refresh Token，OAuth、刷新和钥匙串全部留在官方 [`lark-cli`](https://github.com/larksuite/cli)。详见 [docs/SECURITY.md](docs/SECURITY.md)。
- **PATH 接管默认关闭** —— 不碰你现有的 `lark-cli`，要接管必须显式 `--path-takeover`。

## 30 秒上手

已有官方 `lark-cli` 登录的话，三条命令：

```bash
larkswitch setup          # 装官方 lark-cli + Shim，默认不改 PATH
larkswitch import         # 把已有 ~/.lark-cli 配置收编为第一个人
larkswitch account list   # 看看现在有谁
```

然后二选一：

- **人类**：打开桌面端，托盘点一下换人（或 `larkswitch account switch <uuid>` 持久切换）；
- **终端 / Agent**：`lark-cli --account alias:zhangsan whoami` 单次指定，不动全局。

需要人类在终端直接敲 `lark-cli` 就走本产品时，再显式打开 PATH 接管：`larkswitch setup --path-takeover`。

<details><summary><b>下载安装包</b>（Windows / macOS 带托盘，Linux 纯 CLI）</summary>

| 平台 | 形态 | 下载 |
| --- | --- | --- |
| Windows 10/11 x64 | 桌面端（托盘 + CLI） | [releases/latest](https://github.com/larkswitch/larkswitch/releases/latest) |
| macOS Intel / Apple Silicon | 桌面端（托盘 + CLI） | [releases/latest](https://github.com/larkswitch/larkswitch/releases/latest) |
| Linux x64 | CLI + Shim（无托盘） | [releases/latest](https://github.com/larkswitch/larkswitch/releases/latest) |

安装包是未签名 Alpha，Windows SmartScreen / macOS Gatekeeper 可能拦截——这是预期行为，不是安装包被篡改。Windows 选「更多信息 → 仍要运行」。

**macOS 若提示「已损坏，无法打开」**：不是真坏了，是苹果给未签名下载打的隔离标记。安装后打开「终端」，复制下面两行执行（只需一次，**不用重启**）：

```bash
xattr -dr com.apple.quarantine /Applications/larkswitch.app
open /Applications/larkswitch.app
```

若只是「无法验证开发者」，右键 `.dmg` 选「打开」往往够用；仍被拦、或下错架构，见下方 FAQ。

不需要预装 Node.js / npm，`setup` 会自动下载并校验官方 CLI。
</details>

## 给 Agent 用

把 [`skills/larkswitch/SKILL.md`](skills/larkswitch/SKILL.md) 交给 Claude Code / Cursor / Codex，它就知道怎么替你选人。

> **让 Agent 自己装**——把这段话贴给它：
>
> ```text
> 安装 larkswitch（github.com/larkswitch/larkswitch），运行 larkswitch setup 和
> larkswitch import，然后读取仓库里的 skills/larkswitch/SKILL.md 注册技能。
> 之后你替我操作飞书时，用 --account 指定身份，不要用官方 --profile 选人。
> 装不上就照 docs/CLI.md 里的「从源码构建」来。
> ```

规则只有三条：先解析、后执行；用 `--account` 选人；**绝不用官方 `--profile` 选人**——那是 App 配置，不是「这个人」。

```bash
larkswitch account search --q "张三"           # 宽松找人
larkswitch account resolve 'alias:zhangsan'   # 严格解析：0 个或多个匹配都报错，绝不猜
lark-cli --account alias:zhangsan whoami      # 单次执行
```

Selector 规则（`resolve` 与 `--account` 完全一致，`search` 宽松）：`id:<uuid>`、`alias:<alias>`、裸值（完整 UUID → 精确 alias → 精确且唯一的 displayName）、App 限定 `app:<appId或唯一label>/<identity>`。找不到报 `LPC_ACCOUNT_NOT_FOUND`，多个匹配报 `LPC_ACCOUNT_AMBIGUOUS`。

优先级：命令开头 `--account`（兼容 `--lpc-account`）> 环境变量 `LARKSWITCH_ACCOUNT`（兼容 `LPC_ACCOUNT`）> 当前 active。只吃 argv 开头连续段，中途或 `--` 之后的参数原样透传官方 CLI；官方 `--profile` 不劫持、原样透传。

## 为什么不用 X？

图例：✅ 支持 ｜ ⁉️ 能做到，但要大量手动折腾 ｜ ❌ 做不到

|  | 退出重登 | 官方 `--profile` | 两台机器 / 两个系统用户 | **larkswitch** |
| --- | :---: | :---: | :---: | :---: |
| 同一个 App 下挂多个人 | ⁉️ 每次几十秒 | ❌ 一个 App 一套身份 | ✅ | ✅ |
| 一条命令一个身份，不动全局 | ❌ | ❌ | ❌ | ✅ |
| 并发跑多个身份不串 | ❌ | ❌ | ⁉️ | ✅ |
| 托盘点一下换人 | ❌ | ❌ | ❌ | ✅ |
| 自己保管 Token | — | — | — | ❌ 全部交给官方 CLI |
| 额外装东西 | ✅ 不用装 | ✅ 不用装 | ❌ | ❌ 要装 |

**什么时候你不该用它**：如果你只有一个飞书账号，也不写飞书应用，那官方 `lark-cli` 原样就够了，别装。larkswitch 是给「一个 App 下需要两个及以上身份」的人做的。

## FAQ

<details><summary>Linux 为什么没有托盘？</summary>

Linux 的发布产物是 CLI + Shim：控制面命令与身份隔离全部可用，桌面托盘后置支持。
</details>

<details><summary>状态和备份放在哪？</summary>

状态目录由 `LPC_HOME` 决定（未设置时用各平台用户数据目录）。桌面端启动时做一次文件级备份，之后每 6 小时自动备份到用户文档目录的 `LarkProfileConsoleBackups`；删除程序目录不影响备份。
</details>

<details><summary>它会改我现有的 lark-cli 吗？</summary>

默认不会。PATH 接管是显式的 `--path-takeover`，`--takeover-npm` 才会替换全局 npm 入口。撤销用 `larkswitch path remove`。
</details>

<details><summary>macOS 提示「已损坏，无法打开」或被系统拦截？</summary>

**不是安装包真坏了**，也不是苹果删了文件。v0.2.0 是**未签名 Alpha**（[Release 说明](https://github.com/larkswitch/lark-switch/releases/tag/v0.2.0)），从浏览器/GitHub 下载会带上隔离标记（quarantine）。macOS 对未签名应用有时会显示「**已损坏，无法打开**」——这和「**无法验证开发者**」不是同一句：后者右键安装包选「打开」往往够用；**「已损坏」通常要去掉隔离属性**。

安装后 App 名为 `larkswitch.app`，路径是 `/Applications/larkswitch.app`（`apps/desktop/src-tauri/tauri.conf.json` 里 `productName` 为 `larkswitch`）。

在终端执行（只需一次，**不用重启**）：

```bash
xattr -dr com.apple.quarantine /Applications/larkswitch.app
open /Applications/larkswitch.app
```

若仍被拦：打开 **系统设置 → 隐私与安全性**，在页面底部找「仍要打开」或允许 `larkswitch`。

**下错架构**也可能打不开，但更像闪退或提示不支持，而不是「已损坏」：

| 你的 Mac | 应下载 |
| --- | --- |
| Apple Silicon（M1/M2/M3…） | `larkswitch_0.2.0_aarch64.dmg` |
| Intel | `larkswitch_0.2.0_x64.dmg` |

检查芯片：`uname -m`（`arm64` = Apple Silicon，`x86_64` = Intel）。
</details>

## 文档

- **[完整命令参考 →](docs/CLI.md)**（含从源码构建）
- [安全模型](docs/SECURITY.md)｜[技术架构](docs/ARCHITECTURE.md)｜[产品定义](docs/PRODUCT.md)
- [测试与发布门禁](docs/TESTING.md)｜[人工端到端测试](docs/MANUAL-E2E.md)｜[发布流程](docs/RELEASE.md)

License: [MIT](LICENSE)

---

<sub>本项目为非官方第三方工具，与字节跳动 / 飞书 / Lark 无任何关联，未获其背书或赞助。Lark、飞书为字节跳动的商标，此处仅作描述性使用。本工具不绕过任何鉴权，不保存也不解析用户 Token，OAuth 与钥匙串全部由官方 lark-cli 负责。</sub>
