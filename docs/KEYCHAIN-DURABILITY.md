# 官方 CLI Keychain 持久化与恢复

## 问题本质

| 层 | 存什么 | 清空后果 |
| --- | --- | --- |
| LPC `catalog.json` | 账号元数据、config 路径 | 名单丢 |
| 官方 CLI keychain（Windows 注册表） | App Secret 引用 + 用户 UAT/refresh（DPAPI） | **所有人掉线**，名单仍在 |

OAuth **不能**永久本地化。可做到：

1. **检测** keychain 被整表清空  
2. **自动导出** 注册表快照（滚动保留）  
3. **单条恢复** 脚本（写前先导出安全快照，再把**点名的那一条值**写回）  
4. **用时刷新** 仍由官方 CLI 负责  

## 快照滚动保留

自动导出的快照是滚动保留的：超出上限的旧快照会被清理。**保留份数由 `lpc-core/src/keychain_guard.rs` 中的常量 `MAX_RETAINED_KEYCHAIN_BACKUPS` 决定**——本文刻意不写死数字，调整上限时改常量并跑 `cargo test -p lpc-core`，不要靠文档记忆。

## 历史事故复盘（2026-07-17 / 2026-07-22）

- **2026-07-17**：多账号集中刷新后，一系列调试/恢复操作失误，keychain 从满槽（当天最后一次备份仍有 9 槽）**直接变 0 槽**，全员掉线。当时用一份快照做整表回灌（whole-hive replay），各账号再次通过 `whoami --as user` 与真实 API 调用——那一次只是运气好。
- **2026-07-22**：同样的整表回灌把健康账号的 refresh token 覆盖成快照里的旧值，触发飞书 20064 连环吊销，官方 CLI 直接删条目。

> ⚠️ **整表回灌的做法已作废**。现在只允许单条写回，见下文《运维命令》与《禁止事项》。

## 自动防护（代码）

| 机制 | 位置 |
| --- | --- |
| `inspect_keychain()` 计数槽位（不读密文） | `lpc-core/src/keychain_guard.rs` |
| 上次槽位数落在 `data/keychain-watch.json`（不进 catalog） | `lpc-core/src/keychain_watch.rs` |
| 槽位断崖（例如 15→4，不是空表）打 error，`official_cli_keychain` 为 fail；当轮不做全量 `--verify` | 同上 + `diagnostics.rs` |
| 槽位回升后强制复检仍显示需要登录的账号 | `keychain_watch.rs` / 桌面体检循环 |
| `backup_keychain_registry(reason)` → `Documents\...\keychain\*.reg` | `keychain_guard.rs` |
| 每次文件凭据备份后附带 keychain 导出 | `backup.rs` |
| 桌面启动：空表或断崖 `error` 日志 + 立即导出 | `apps/desktop/.../main.rs` |
| 每 6h 备份线程：空表告警 | 同上 |
| `lpcctl doctor`：`official_cli_keychain` 检查 | `diagnostics.rs` |
| `lpcctl doctor`：`autostart_target`（开机项是否指向安装目录、是否 cargo target） | `autostart.rs` / `diagnostics.rs` |

## 运维命令

恢复统一走 `scripts/restore-lark-keychain.ps1`。**默认是干跑（只列不写）**，必须用 `-ValueName` 逐条点名、再加 `-Apply` 才会真正落盘。

```powershell
# 1) 干跑：列出最新非空快照里的每个值，标出 new / same / differs。一个字节都不写
powershell -ExecutionPolicy Bypass -File scripts\restore-lark-keychain.ps1

# 2) 干跑 + 指定快照
powershell -ExecutionPolicy Bypass -File scripts\restore-lark-keychain.ps1 `
  -Snapshot "$env:USERPROFILE\Documents\LarkProfileConsoleBackups\keychain\<YYYYMMDD-HHMMSS>-<label>.reg"

# 3) 真写回：逐条点名 + -Apply。脚本会先把当前 keychain 导成安全快照，导不出就直接 throw
powershell -ExecutionPolicy Bypass -File scripts\restore-lark-keychain.ps1 `
  -Snapshot "<快照路径>" -ValueName "<base64url(appId:userOpenId)>" -Apply
```

| 参数 | 含义 |
| --- | --- |
| `-Snapshot` | 快照路径；默认 `latest-nonempty`，即备份目录里最新的非空快照 |
| `-ValueName` | 要写回的注册表值名，可给多个。**不传 = 只读报告**；脚本刻意没有「一键全恢复」开关 |
| `-Apply` | 不加就是干跑。加上才写回，且写前强制导出安全快照 |
| `-ProbeAccounts` | 写回成功后跑 `whoami --as user` 验证（干跑时不执行，避免无谓地轮换 token） |

验收（必须 `--as user`）：

```powershell
$env:LPC_HOME = "$env:LOCALAPPDATA\LarkProfileConsole\Lark Profile Console\data"
foreach ($n in '<本次写回的账号标签1>','<本次写回的账号标签2>') {
  lark-cli --lpc-account $n whoami --as user
}
```

## 禁止事项

1. 调试时 `Remove-ItemProperty` 删 UAT 却不恢复。  
2. **整表回灌**（whole-hive replay）：拿一份快照覆盖整个 keychain。飞书 refresh token 一次一换，快照里的旧值早就作废，回灌会连健康账号一起打死（20064 连环吊销，2026-07-22 实锤）。只能单条写回。  
3. **删除或重建 `HKCU\Software\LarkCli\keychain` 键本身**。键级操作一旦中途失败就是空仓、全员掉线；只允许对单个值做 `Set-ItemProperty` / `Remove-ItemProperty`。  
4. 任何写操作前不导 `.reg` 安全快照。  
5. 用裸 `whoami` 判断用户是否登录（会回落 bot）。  
6. 指望「自动刷新」在 **注册表被清空** 后自愈——必须从快照里逐条写回，或重新授权。  

## 槽位断崖（2026-08-13）

空表不是唯一事故形态。钥匙串从 15 槽降到 4 槽时 `empty` 仍为 false，旧逻辑不会告警。现在会把上次观察到的 `entry_count` 记在数据目录的 `keychain-watch.json`（不要往 `catalog.json` 加字段）。

出现断崖时：

- 日志事件 `keychain slot cliff`
- 桌面 Toast / 托盘提示
- `lpcctl doctor` 的 `official_cli_keychain` 为 fail
- **当轮不对全部账号跑 `--verify`**。官方 CLI 拿作废 refresh 去换票会再删条目

恢复仍只允许 `scripts/restore-lark-keychain.ps1`：默认干跑，按快照里的 `new` 槽用 `-ValueName` 逐条点名，再加 `-Apply`。不要对仍显示健康的槽写回。槽位相对上次上升后，控制平面会对仍为需要登录的账号强制 `status --verify`，避免界面停留在恢复前的结论。

## Workspace Gateway

见 `local-workbench-bridge/docs/LARK-USER-TOKEN-GATE.md`：业务前强制 `whoami --as user`。
