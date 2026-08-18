[简体中文](README.md) · [Docs](docs/) · [Download](https://github.com/larkswitch/larkswitch/releases/latest)

# larkswitch

**Run many accounts under one Feishu/Lark App, and pick who each command runs as — no logging out and back in.**

[![Release](https://img.shields.io/github/v/release/larkswitch/larkswitch?include_prereleases&label=release)](https://github.com/larkswitch/larkswitch/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-informational.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-informational)](https://github.com/larkswitch/larkswitch/releases/latest)

<sub>Unofficial third-party tool. Not affiliated with ByteDance / Feishu / Lark.</sub>

<!-- Replace with a 15-20s real screen recording before launch (see docs/assets/demo.tape),
     640-800px wide, <3MB. Delete this comment and the switch.svg line once the GIF lands. -->
![Switching from the tray affects only the next command; in-flight commands keep the identity they started with](docs/assets/switch.svg)

<sub>What to watch: after the tray switches to Li Si, **newly started commands** run as Li Si while the **in-flight command** is still Zhang San.</sub>

```bash
# One identity per command; the global active account is untouched
lark-cli --account alias:bot whoami                      # the bot account
lark-cli --account alias:me  whoami                      # you
lark-cli --account alias:me  calendar +agenda --as user
```

## What is it?

Official `lark-cli --profile` manages an **App configuration**. Since v1.0.5 it can manage several Apps, but **a single App ID still holds exactly one identity**.

Anyone building a Feishu/Lark app hits this wall: testing needs at least two accounts — one bot, one human. Consultants and agencies also hop between several customer tenants. Today's workaround is log out, log in, log out, log in.

larkswitch uses the officially supported `LARKSUITE_CLI_CONFIG_DIR` to isolate every `(App, User)` pair into its own config directory, and decides who the next command runs as:

- **One App, many people** — each account gets an isolated config directory; they never pollute each other.
- **Per-command identity** — `--account` affects that one command only; the global active account does not move.
- **In-flight commands never change identity** — identity is snapshotted when a command starts. Running several identities concurrently never crosses wires.
- **Tokens never touched** — no Access / Refresh Token storage. OAuth, refresh and the OS keychain stay with official [`lark-cli`](https://github.com/larksuite/cli). See [docs/SECURITY.md](docs/SECURITY.md).
- **PATH takeover off by default** — your existing `lark-cli` is untouched unless you explicitly pass `--path-takeover`.

## 30-second quickstart

If you already logged in with the official `lark-cli`, three commands:

```bash
larkswitch setup          # install official lark-cli + the shim; PATH untouched by default
larkswitch import         # absorb your existing ~/.lark-cli config as the first person
larkswitch account list   # see who is here
```

Then pick one:

- **Humans**: open the desktop app and switch from the tray (or `larkswitch account switch <uuid>` for a persistent switch);
- **Terminal / agents**: `lark-cli --account alias:zhangsan whoami` for a one-shot, without touching the global active.

If you want a plain `lark-cli` in your terminal to route through this product, turn PATH takeover on explicitly: `larkswitch setup --path-takeover`.

<details><summary><b>Download installers</b> (Windows / macOS with tray, Linux CLI-only)</summary>

| Platform | Form factor | Download |
| --- | --- | --- |
| Windows 10/11 x64 | Desktop app (tray + CLI) | [releases/latest](https://github.com/larkswitch/larkswitch/releases/latest) |
| macOS Intel / Apple Silicon | Desktop app (tray + CLI) | [releases/latest](https://github.com/larkswitch/larkswitch/releases/latest) |
| Linux x64 | CLI + shim (no tray) | [releases/latest](https://github.com/larkswitch/larkswitch/releases/latest) |

The installers are an unsigned alpha, so Windows SmartScreen / macOS Gatekeeper may block them — that is expected, not a sign of tampering. Windows: "More info" → "Run anyway". macOS: right-click the package and choose "Open", or allow it in System Settings.

No Node.js / npm required; `setup` downloads and verifies the official CLI for you.
</details>

## For agents

Hand [`skills/larkswitch/SKILL.md`](skills/larkswitch/SKILL.md) to Claude Code / Cursor / Codex and it knows how to pick people for you.

> **Let the agent install it** — paste this to it:
>
> ```text
> Install larkswitch (github.com/larkswitch/larkswitch), run `larkswitch setup` and
> `larkswitch import`, then read skills/larkswitch/SKILL.md in the repo and register the skill.
> From then on, when you act on Feishu/Lark for me, pick the identity with --account.
> Never pick a person with the official --profile. If setup fails, follow the
> "Build from source" section in docs/CLI.md.
> ```

Three rules: resolve first, then run; pick people with `--account`; **never pick a person with official `--profile`** — that is an App configuration, not "the person".

```bash
larkswitch account search --q "Zhang"         # loose lookup
larkswitch account resolve 'alias:zhangsan'   # strict: 0 or multiple matches error out, never guess
lark-cli --account alias:zhangsan whoami      # one-shot execution
```

Selector grammar (`resolve` and `--account` are identical; `search` is loose): `id:<uuid>`, `alias:<alias>`, bare value (full UUID → exact alias → exact and unique displayName), App-scoped `app:<appId or unique label>/<identity>`. No match → `LPC_ACCOUNT_NOT_FOUND`; multiple matches → `LPC_ACCOUNT_AMBIGUOUS`.

Precedence: leading `--account` (compat `--lpc-account`) > env `LARKSWITCH_ACCOUNT` (compat `LPC_ACCOUNT`) > current active. Only a leading run of argv is consumed; anything mid-argv or after `--` passes through to the official CLI untouched. Official `--profile` is never hijacked and passes through as-is.

## Why not X?

Legend: ✅ supported ｜ ⁉️ possible with significant manual effort ｜ ❌ not supported

|  | Log out / log in | Official `--profile` | Two machines / two OS users | **larkswitch** |
| --- | :---: | :---: | :---: | :---: |
| Many people under one App | ⁉️ tens of seconds each time | ❌ one identity per App | ✅ | ✅ |
| One identity per command, global untouched | ❌ | ❌ | ❌ | ✅ |
| Concurrent identities never cross wires | ❌ | ❌ | ⁉️ | ✅ |
| Switch from a tray | ❌ | ❌ | ❌ | ✅ |
| You hold the tokens | — | — | — | ❌ all left to the official CLI |
| Extra software to install | ✅ none | ✅ none | ❌ | ❌ yes |

**When you should not use it**: if you have exactly one Feishu/Lark account and you do not build Feishu/Lark apps, plain `lark-cli` is enough — don't install this. larkswitch exists for people who need two or more identities under one App.

## FAQ

<details><summary>Why is there no tray on Linux?</summary>

The Linux release ships CLI + shim: every control-plane command and the identity isolation work; the desktop tray is planned later.
</details>

<details><summary>Where do state and backups live?</summary>

The state directory is decided by `LPC_HOME` (platform user-data directory when unset). The desktop app takes a file-level backup at startup and then every 6 hours into `LarkProfileConsoleBackups` under your Documents folder; deleting the program directory does not touch backups.
</details>

<details><summary>Will it change my existing lark-cli?</summary>

Not by default. PATH takeover is an explicit `--path-takeover`; only `--takeover-npm` replaces the global npm entry. Undo with `larkswitch path remove`.
</details>

<details><summary>macOS says "damaged" or blocks the app?</summary>

**The installer is not actually corrupted** and Apple did not delete it. v0.2.0 is an **unsigned alpha** ([release notes](https://github.com/larkswitch/lark-switch/releases/tag/v0.2.0)); downloads from a browser/GitHub carry a quarantine flag. macOS sometimes shows **"larkswitch is damaged and can't be opened"** for unsigned apps — that is different from **"cannot verify the developer"**: the latter is often fixed by right-click → Open on the installer; **"damaged" usually means you must clear the quarantine attribute**.

After install the app is `larkswitch.app` at `/Applications/larkswitch.app` (`productName` is `larkswitch` in `apps/desktop/src-tauri/tauri.conf.json`).

Run once in Terminal (**no reboot needed**):

```bash
xattr -dr com.apple.quarantine /Applications/larkswitch.app
open /Applications/larkswitch.app
```

If it is still blocked: **System Settings → Privacy & Security**, scroll down and choose **Open Anyway** or allow `larkswitch`.

**Wrong architecture** can also fail to launch, but that usually crashes or says unsupported — not "damaged":

| Your Mac | Download |
| --- | --- |
| Apple Silicon (M1/M2/M3…) | `larkswitch_0.2.0_aarch64.dmg` |
| Intel | `larkswitch_0.2.0_x64.dmg` |

Check chip: `uname -m` (`arm64` = Apple Silicon, `x86_64` = Intel).
</details>

## Documentation

- **[Full command reference →](docs/CLI.md)** (including build from source)
- [Security model](docs/SECURITY.md) ｜ [Architecture](docs/ARCHITECTURE.md) ｜ [Product definition](docs/PRODUCT.md)
- [Testing & release gates](docs/TESTING.md) ｜ [Manual end-to-end testing](docs/MANUAL-E2E.md) ｜ [Release process](docs/RELEASE.md)

License: [MIT](LICENSE)

---

<sub>This is an unofficial third-party tool. It is not affiliated with, endorsed by, or sponsored by ByteDance / Feishu / Lark. Lark and Feishu are trademarks of ByteDance, used here descriptively only. This tool circumvents no authentication and neither stores nor parses user tokens; OAuth and the keychain are handled entirely by the official lark-cli.</sub>
