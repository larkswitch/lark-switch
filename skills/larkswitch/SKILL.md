---
name: larkswitch
description: Route lark-cli operations through the intended larkswitch user account. Use whenever an AI needs to list, search, resolve, select, or switch Feishu/Lark accounts; act on behalf of a named person; choose among multiple authorized users; verify the executing identity; or sees account, 账号, 身份, 代某人, 授权用户, larkswitch, lpcctl, --account, --lpc-account, LARKSWITCH_ACCOUNT, or LPC_ACCOUNT. Also use before a lark-cli business command when the user names the account or person to operate as.
---

# larkswitch routing

Treat a larkswitch account as a **person**. The official CLI Profile is an App configuration. Never use official `--profile` to select a person. Select a person with `--account` (compatibility alias `--lpc-account`).

## Choose the account

`larkswitch` and `lpcctl` are the same control plane. Prefer `larkswitch`. If it is not on `PATH`, use the helper:

```powershell
$larkswitch = "$HOME\.agents\skills\larkswitch\scripts\larkswitch.ps1"
& $larkswitch account list
& $larkswitch account search --q "name or alias"
& $larkswitch account resolve "exact selector"
```

1. If the user names a person, account, or alias, resolve it before the business command.
2. Use `search` only for discovery. Use `resolve` for execution; never guess after zero or multiple matches.
3. Accept strict selectors: `id:<uuid>`, `alias:<alias>`, an exact unique display name, or `app:<app-id-or-unique-label>/<identity>`.
4. If the user does not specify an account, use the current locked account without switching it.

## Execute without changing the default

Put `--account` at the very beginning of the `lark-cli` arguments:

```powershell
lark-cli --account "exact selector" whoami
lark-cli --account "exact selector" calendar +agenda --as user
```

`--as user` is official user-vs-bot, not a person selector. Prefer the one-shot flag for AI calls. It affects only that process.

For several commands in one controlled terminal session, `LARKSWITCH_ACCOUNT` is allowed (`LPC_ACCOUNT` is a compatibility alias). Clear it afterward.

Priority: `--account` / `--lpc-account` over `LARKSWITCH_ACCOUNT` over `LPC_ACCOUNT` over the current locked account.

## Verify identity safely

```powershell
lark-cli --account "exact selector" whoami
```

Confirm `onBehalfOf.userName` matches the intended person. Do not print tokens, App secrets, keychain data, account config directories, or complete diagnostic dumps.

## Change the default only when explicitly requested

```powershell
& $larkswitch account resolve "exact selector"
& $larkswitch account switch "resolved account UUID"
```

Already-running commands keep their startup identity.

## Avoid these errors

- Wrong: `lark-cli --profile "person" ...`
- Wrong: globally switch accounts for a one-off request.
- Wrong: choose the first fuzzy search result.
- Correct: `lark-cli --account "person or alias" ...`
