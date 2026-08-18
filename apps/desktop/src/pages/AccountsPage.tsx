import { useEffect, useMemo, useRef, useState } from 'react';
import { Banner, Button, Empty, Input, Popconfirm, Skeleton, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { IconPlus, IconSearch } from '@douyinfe/semi-icons';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { accountCommand, validateAccountAlias } from '../account-selector';
import { api } from '../api';
import { maskAppId } from '../auth-presentation';
import { HealthTag } from '../components/HealthTag';
import { PageHeader } from '../components/PageHeader';
import { copy } from '../copy';
import type { AccountRecord, AccountView, Snapshot } from '../types';
import { formatTime, normalizeError } from '../utils';

const { Title, Text } = Typography;

export interface AccountsPageProps {
  data: Snapshot;
  onSwitch: (id: string) => Promise<void>;
  onAdd: () => void;
  onImportLocal: () => void;
  onReauth: (id: string) => Promise<void>;
  onReload: () => Promise<void>;
}

interface AccountGroup {
  appId: string;
  label: string;
  items: AccountView[];
}

type PendingAction = 'alias-save' | 'alias-clear' | 'check' | 'remove' | 'copy' | null;

function matchesQuery(item: AccountView, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return [item.account.displayName, item.account.alias ?? '', item.app.label]
    .some((value) => value.toLowerCase().includes(needle));
}

function groupAccountsByApp(accounts: AccountView[]): AccountGroup[] {
  const groups: AccountGroup[] = [];
  const indexByApp = new Map<string, number>();
  for (const item of accounts) {
    const appId = item.app.id;
    const existing = indexByApp.get(appId);
    if (existing === undefined) {
      indexByApp.set(appId, groups.length);
      groups.push({ appId, label: item.app.label, items: [item] });
    } else {
      groups[existing].items.push(item);
    }
  }
  return groups;
}

function defaultSelectedId(accounts: AccountView[]): string | null {
  return accounts.find((item) => item.active)?.account.id ?? accounts[0]?.account.id ?? null;
}

function aliasHintFor(
  draft: string,
  currentAlias: string | null | undefined,
  others: AccountRecord[],
): { error: boolean; text: string; canSave: boolean } {
  const result = validateAccountAlias(draft);
  if (!result.ok) {
    if (result.reason === 'empty') {
      return { error: false, text: copy.accounts.aliasHint, canSave: false };
    }
    const text = result.reason === 'too_long'
      ? copy.accounts.aliasInvalidTooLong
      : result.reason === 'bad_chars'
        ? copy.accounts.aliasInvalidChars
        : copy.accounts.aliasInvalidPrefix;
    return { error: true, text, canSave: false };
  }
  if (others.some((account) => account.alias === result.alias)) {
    return { error: true, text: copy.accounts.aliasDuplicate, canSave: false };
  }
  return {
    error: false,
    text: copy.accounts.aliasHint,
    canSave: result.alias !== (currentAlias ?? ''),
  };
}

interface AccountListItemProps {
  item: AccountView;
  selected: boolean;
  onSelect: (id: string) => void;
}

function AccountListItem(props: AccountListItemProps) {
  const { item } = props;
  return (
    <button
      type="button"
      className={`accounts-list-item${props.selected ? ' selected' : ''}`}
      onClick={() => props.onSelect(item.account.id)}
    >
      <div className="accounts-list-item-copy">
        <span className="accounts-list-item-name">{item.account.displayName}</span>
        {item.account.alias ? (
          <span className="accounts-list-item-alias">{item.account.alias}</span>
        ) : null}
      </div>
      <div className="accounts-list-item-tags">
        {item.active ? <Tag color="green">{copy.accounts.currentTag}</Tag> : null}
        <HealthTag health={item.account.health} running={0} />
        {item.runningCommands > 0 ? (
          <Tag color="blue">{copy.health.running(item.runningCommands)}</Tag>
        ) : null}
      </div>
    </button>
  );
}

function AccountsListSkeleton() {
  return (
    <div className="accounts-list-skeleton">
      {Array.from({ length: 5 }, (_, index) => (
        <div className="accounts-skeleton-row" key={index}>
          <Skeleton.Title style={{ width: '70%', height: 16 }} />
          <Skeleton.Paragraph rows={1} style={{ width: '46%', marginTop: 8 }} />
        </div>
      ))}
    </div>
  );
}

interface AccountDetailProps {
  item: AccountView;
  allAccounts: AccountRecord[];
  pending: PendingAction;
  aliasDraft: string;
  onAliasDraftChange: (value: string) => void;
  onSaveAlias: () => Promise<void>;
  onClearAlias: () => Promise<void>;
  onCopyCommand: () => Promise<void>;
  onCheck: () => Promise<void>;
  onSwitch: (id: string) => Promise<void>;
  onReauth: (id: string) => Promise<void>;
  onRemove: () => Promise<void>;
}

function AccountDetail(props: AccountDetailProps) {
  const { item, pending, aliasDraft } = props;
  const others = props.allAccounts.filter((account) => account.id !== item.account.id);
  const aliasHint = aliasHintFor(aliasDraft, item.account.alias, others);
  const command = accountCommand(item.account, props.allAccounts);
  const busy = pending !== null;
  const tenant = item.account.tenantLabel?.trim();

  return (
    <div className="accounts-detail">
      <div className="accounts-detail-header">
        <div className="accounts-detail-title-row">
          <Title heading={3}>{item.account.displayName}</Title>
          <div className="accounts-list-item-tags">
            {item.active ? <Tag color="green">{copy.accounts.currentTag}</Tag> : null}
            <HealthTag health={item.account.health} running={0} />
          </div>
        </div>
      </div>

      <div className="accounts-detail-meta">
        <div className="accounts-detail-row">
          <Text type="tertiary">{copy.accounts.appLabel}</Text>
          <Text>{item.app.label}</Text>
        </div>
        {tenant ? (
          <div className="accounts-detail-row">
            <Text type="tertiary">{copy.accounts.tenantLabel}</Text>
            <Text>{tenant}</Text>
          </div>
        ) : null}
        <div className="accounts-detail-row">
          <Text type="tertiary">{copy.accounts.openId(maskAppId(item.account.userOpenId))}</Text>
        </div>
        <div className="accounts-detail-row">
          <Text type="tertiary">{copy.accounts.lastVerified(formatTime(item.account.lastVerifiedAt))}</Text>
        </div>
        <div className="accounts-detail-row">
          <Text type="tertiary">{copy.accounts.scopeCount(item.account.effectiveScopes.length)}</Text>
        </div>
      </div>

      <div className="accounts-alias-editor">
        <Text strong>{copy.accounts.alias}</Text>
        <Input
          className="full-width"
          value={aliasDraft}
          placeholder={copy.accounts.aliasPlaceholder}
          aria-label={copy.accounts.alias}
          aria-describedby="account-alias-hint"
          validateStatus={aliasHint.error ? 'error' : 'default'}
          showClear
          onChange={props.onAliasDraftChange}
          onEnterPress={() => {
            if (aliasHint.canSave && !busy) void props.onSaveAlias();
          }}
        />
        <div className="accounts-alias-actions">
          <Button
            disabled={!aliasHint.canSave || busy}
            loading={pending === 'alias-save'}
            onClick={() => void props.onSaveAlias()}
          >
            {copy.accounts.aliasSave}
          </Button>
          <Button
            theme="borderless"
            disabled={!item.account.alias || busy}
            loading={pending === 'alias-clear'}
            onClick={() => void props.onClearAlias()}
          >
            {copy.accounts.aliasClear}
          </Button>
        </div>
        <Text
          id="account-alias-hint"
          type={aliasHint.error ? 'danger' : 'tertiary'}
          className={aliasHint.error ? 'accounts-alias-hint invalid' : 'accounts-alias-hint'}
        >
          {aliasHint.text}
        </Text>
      </div>

      <div className="accounts-command">
        <Text strong>{copy.accounts.commandLabel}</Text>
        <pre className="accounts-command-text">{command}</pre>
        <Button
          theme="borderless"
          disabled={busy}
          loading={pending === 'copy'}
          onClick={() => void props.onCopyCommand()}
        >
          {copy.accounts.copyCommand}
        </Button>
      </div>

      {item.runningCommands > 0 ? (
        <Banner
          className="inline-banner"
          type="warning"
          description={copy.accounts.runningBanner(item.runningCommands)}
        />
      ) : null}

      <div className="accounts-detail-actions">
        {item.active ? (
          <Button theme="borderless" disabled>
            {copy.accounts.using}
          </Button>
        ) : (
          <Button theme="solid" disabled={busy} onClick={() => void props.onSwitch(item.account.id)}>
            {copy.accounts.switchTo}
          </Button>
        )}
        <Button
          theme={item.active ? 'solid' : 'light'}
          disabled={busy}
          onClick={() => void props.onReauth(item.account.id)}
        >
          {copy.accounts.reauth}
        </Button>
        <Button
          theme="borderless"
          disabled={busy}
          loading={pending === 'check'}
          onClick={() => void props.onCheck()}
        >
          {copy.accounts.check}
        </Button>
        <Popconfirm
          title={copy.accounts.removeTitle}
          content={item.account.credentialOrigin === 'external_shared'
            ? copy.accounts.removeExternal
            : copy.accounts.removeManaged}
          onConfirm={() => void props.onRemove()}
        >
          <Button type="danger" disabled={busy} loading={pending === 'remove'}>
            {copy.accounts.remove}
          </Button>
        </Popconfirm>
      </div>
    </div>
  );
}

export function AccountsPage(props: AccountsPageProps) {
  const accounts = props.data.accounts;
  const [query, setQuery] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(() => defaultSelectedId(accounts));
  const [aliasDraft, setAliasDraft] = useState('');
  const [pending, setPending] = useState<PendingAction>(null);
  const pendingRef = useRef(false);

  const records = useMemo(() => accounts.map((item) => item.account), [accounts]);
  const filtered = useMemo(
    () => accounts.filter((item) => matchesQuery(item, query)),
    [accounts, query],
  );
  const groups = useMemo(() => groupAccountsByApp(filtered), [filtered]);
  const selected = accounts.find((item) => item.account.id === selectedId) ?? null;

  useEffect(() => {
    if (selectedId && accounts.some((item) => item.account.id === selectedId)) return;
    setSelectedId(defaultSelectedId(accounts));
  }, [accounts, selectedId]);

  useEffect(() => {
    setAliasDraft(selected?.account.alias ?? '');
  }, [selected?.account.id, selected?.account.alias]);

  const run = async (action: Exclude<PendingAction, null>, work: () => Promise<void>) => {
    if (pendingRef.current) return;
    pendingRef.current = true;
    setPending(action);
    try {
      await work();
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      pendingRef.current = false;
      setPending(null);
    }
  };

  return (
    <section>
      <PageHeader
        title={copy.accounts.title}
        description={copy.accounts.description}
        detailHeader={copy.accounts.detailHeader}
        detail={copy.accounts.detail}
        action={(
          <div className="card-actions">
            <Button onClick={props.onImportLocal}>{copy.accounts.importLocal}</Button>
            <Button icon={<IconPlus />} theme="solid" onClick={props.onAdd}>{copy.accounts.add}</Button>
          </div>
        )}
      />
      {accounts.length === 0 ? (
        <Empty title={copy.accounts.emptyTitle} description={copy.accounts.emptyDescription} />
      ) : (
        <div className="accounts-split">
          <div className="accounts-list-pane">
            <div className="accounts-search">
              <Input
                className="full-width"
                prefix={<IconSearch />}
                showClear
                composition
                placeholder={copy.accounts.searchPlaceholder}
                aria-label={copy.accounts.searchPlaceholder}
                value={query}
                onChange={setQuery}
              />
            </div>
            <div className="accounts-list-body">
              <Skeleton
                loading={pending === 'check'}
                active
                placeholder={<AccountsListSkeleton />}
              >
                {filtered.length === 0 ? (
                  <Empty title={copy.accounts.noMatch} />
                ) : (
                  <div className="accounts-list">
                    {groups.map((group) => (
                      <div className="accounts-group" key={group.appId}>
                        <div className="accounts-group-title">{group.label}</div>
                        {group.items.map((item) => (
                          <AccountListItem
                            key={item.account.id}
                            item={item}
                            selected={item.account.id === selectedId}
                            onSelect={setSelectedId}
                          />
                        ))}
                      </div>
                    ))}
                  </div>
                )}
              </Skeleton>
            </div>
          </div>
          <div className="accounts-detail-pane">
            {selected ? (
              <AccountDetail
                item={selected}
                allAccounts={records}
                pending={pending}
                aliasDraft={aliasDraft}
                onAliasDraftChange={setAliasDraft}
                onSaveAlias={() => run('alias-save', async () => {
                  const result = validateAccountAlias(aliasDraft);
                  if (!result.ok) return;
                  await api.setAccountAlias(selected.account.id, result.alias);
                  Toast.success(copy.accounts.aliasSaved);
                  await props.onReload();
                })}
                onClearAlias={() => run('alias-clear', async () => {
                  await api.clearAccountAlias(selected.account.id);
                  Toast.success(copy.accounts.aliasCleared);
                  await props.onReload();
                })}
                onCopyCommand={() => run('copy', async () => {
                  await writeText(accountCommand(selected.account, records));
                  Toast.success(copy.accounts.commandCopied);
                })}
                onCheck={() => run('check', async () => {
                  await api.checkAccount(selected.account.id);
                  await props.onReload();
                  Toast.success(copy.accounts.checked);
                })}
                onSwitch={props.onSwitch}
                onReauth={props.onReauth}
                onRemove={() => run('remove', async () => {
                  await api.removeAccount(selected.account.id);
                  Toast.success(copy.accounts.removed);
                  await props.onReload();
                })}
              />
            ) : (
              <Empty className="accounts-detail-empty" title={copy.accounts.detailEmpty} />
            )}
          </div>
        </div>
      )}
    </section>
  );
}
