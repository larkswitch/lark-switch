# Manual Real-Account E2E Checklist

Use a disposable test App. Never paste production secrets into CI logs.

## Preparation

```bash
export LPC_HOME=/absolute/disposable/path
export LPC_TEST_APP_ID=cli_xxx
export LPC_TEST_APP_SECRET=...
./scripts/real-oauth-e2e.sh
```

Record App ID suffixes and masked Open IDs only.

## Same App, two users

- Add user A.
- Add user B with the same App record.
- Confirm the account directories differ.
- Confirm both configs refer to the same App ID and keychain reference.
- Confirm `lpcctl account list` contains distinct Open IDs.
- Confirm compact list/search/resolve JSON has no `configDir` / secret / token / deviceCode / keychain fields.
- Set unique aliases with `lpcctl account alias set`; confirm `lpcctl account resolve alias:<name>` and bare alias resolve.
- With at least two real accounts: record global active + `active-state.json` bytes/generation; run `lark-cli --lpc-account id:<B> whoami` and `LPC_ACCOUNT=id:<A> lark-cli whoami`; confirm openId matches the selected account and global active is unchanged.
- Confirm official passthrough: `lark-cli --profile <name> ...` and `lark-cli profile list` are not rewritten by LPC.
- Confirm leading unknown `--lpc-foo` exits 64 with `LPC_ACCOUNT_SELECTOR_INVALID`, while mid-argv `--lpc-account` is forwarded.

## Mid-task switch semantics

1. Select A.
2. Start a real long-running/read loop through product Shim.
3. Switch tray to B while A's process is still running.
4. Confirm UI says the old command remains on A.
5. Run a new `lark-cli whoami`; it must report B.
6. Confirm the old loop still reports/acts as A.

## Permissions

- Save a stable subset of live scopes.
- Add a new permission to the App console.
- Refresh App boundary: the new scope must appear unselected.
- Add/reauthorize an account: request must contain policy only.
- For multi-batch plans, close/restart during a batch; the old device code must not resume.
- Complete all batches; final target minus actual must be empty.

## Failure injection

- Disconnect network during status check: do not relabel immediately as revoked.
- Corrupt active state JSON: Shim must refuse to guess.
- Kill Shim child/parent: orphan lease cleanup must recover by PID/start time.
- Delete active runtime binary: Shim returns stable runtime-missing error.
- Feed a modified archive/checksum in a local test harness: activation must fail.
- Attempt account deletion while it has a running lease: deletion must be blocked.

## Removal

- Remove one user under a shared App.
- Verify other users under the App still work.
- Remove PATH takeover.
- Verify only the managed entry/block disappears and unrelated user edits remain.
