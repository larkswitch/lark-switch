import { useEffect, useMemo, useState } from 'react';
import {
  Banner,
  Button,
  Checkbox,
  Collapse,
  Empty,
  Input,
  Popconfirm,
  SideSheet,
  Space,
  Spin,
  Toast,
  Typography,
} from '@douyinfe/semi-ui';
import { IconSearch } from '@douyinfe/semi-icons';
import { api } from '../api';
import { copy } from '../copy';
import { MAX_SINGLE_AUTH_SCOPES } from '../limits';
import type { AppRecord, ScopeInfo } from '../types';
import { normalizeError } from '../utils';

const { Paragraph, Text } = Typography;

export interface ScopePolicyDrawerProps {
  app: AppRecord;
  visible: boolean;
  onClose: () => void;
  onSaved: () => void | Promise<void>;
  onGoAccounts: () => void;
}

interface ScopeModuleGroup {
  module: string;
  items: ScopeInfo[];
}

interface ScopeOptionRowProps {
  item: ScopeInfo;
  checked: boolean;
  onCheckedChange: (scope: string, checked: boolean) => void;
}

function moduleOf(scope: string): string {
  return scope.split(':')[0] || 'other';
}

function sameStringSet(left: Set<string>, right: Set<string>): boolean {
  if (left.size !== right.size) {
    return false;
  }
  for (const item of left) {
    if (!right.has(item)) {
      return false;
    }
  }
  return true;
}

function collapseKeys(value: string | string[] | undefined): string[] {
  if (value == null) {
    return [];
  }
  return (Array.isArray(value) ? value : [value]).filter((key) => key.length > 0);
}

function groupScopeInfos(items: ScopeInfo[]): ScopeModuleGroup[] {
  const groups = new Map<string, ScopeInfo[]>();
  for (const item of items) {
    const module = moduleOf(item.scope);
    const list = groups.get(module) ?? [];
    list.push(item);
    groups.set(module, list);
  }
  return [...groups.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([module, grouped]) => ({
      module,
      items: [...grouped].sort((left, right) => left.scope.localeCompare(right.scope)),
    }));
}

function ScopeOptionRow(props: ScopeOptionRowProps) {
  const [confirming, setConfirming] = useState(false);
  const reason = props.item.reason?.trim() ?? '';
  const extra = !props.item.core && reason ? reason : undefined;

  const checkbox = (
    <Checkbox
      checked={props.checked}
      extra={extra}
      onChange={(event) => {
        const nextChecked = Boolean(event.target.checked);
        if (nextChecked && !props.item.core && !props.checked) {
          setConfirming(true);
          return;
        }
        props.onCheckedChange(props.item.scope, nextChecked);
      }}
    >
      {props.item.scope}
    </Checkbox>
  );

  if (props.item.core) {
    return <div>{checkbox}</div>;
  }

  return (
    <div>
      <Popconfirm
        trigger="custom"
        visible={confirming}
        title={copy.apps.advancedConfirmTitle}
        content={copy.apps.advancedConfirm(reason)}
        onConfirm={() => {
          props.onCheckedChange(props.item.scope, true);
          setConfirming(false);
        }}
        onCancel={() => setConfirming(false)}
        onClickOutSide={() => setConfirming(false)}
      >
        {checkbox}
      </Popconfirm>
    </div>
  );
}

interface ModuleGroupListProps {
  groups: ScopeModuleGroup[];
  selected: Set<string>;
  onCheckedChange: (scope: string, checked: boolean) => void;
}

function ModuleGroupList(props: ModuleGroupListProps) {
  return (
    <>
      {props.groups.map((group) => {
        const enabled = group.items.filter((item) => props.selected.has(item.scope)).length;
        return (
          <div className="scope-group" key={group.module}>
            <div className="scope-group-title">
              {copy.apps.moduleGroup(group.module, enabled, group.items.length)}
            </div>
            {group.items.map((item) => (
              <ScopeOptionRow
                key={item.scope}
                item={item}
                checked={props.selected.has(item.scope)}
                onCheckedChange={props.onCheckedChange}
              />
            ))}
          </div>
        );
      })}
    </>
  );
}

export function ScopePolicyDrawer(props: ScopePolicyDrawerProps) {
  const [catalog, setCatalog] = useState<ScopeInfo[]>([]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set(props.app.policyScopes));
  const [query, setQuery] = useState('');
  const [activeKeys, setActiveKeys] = useState<string[]>(['core']);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!props.visible) {
      return;
    }
    let cancelled = false;
    setQuery('');
    setActiveKeys(['core']);
    setSelected(new Set(props.app.policyScopes));
    setCatalog([]);
    setLoading(true);
    void api.scopeCatalog(props.app.id)
      .then((items) => {
        if (!cancelled) {
          setCatalog(items);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          Toast.error(normalizeError(error));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [props.visible, props.app.id]);

  const saved = useMemo(() => new Set(props.app.policyScopes), [props.app.policyScopes]);
  const dirty = !sameStringSet(selected, saved);
  const overLimit = selected.size > MAX_SINGLE_AUTH_SCOPES;

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) {
      return catalog;
    }
    return catalog.filter((item) => item.scope.toLowerCase().includes(needle));
  }, [catalog, query]);

  const coreItems = useMemo(() => filtered.filter((item) => item.core), [filtered]);
  const advancedItems = useMemo(() => filtered.filter((item) => !item.core), [filtered]);
  const coreGroups = useMemo(() => groupScopeInfos(coreItems), [coreItems]);
  const advancedGroups = useMemo(() => groupScopeInfos(advancedItems), [advancedItems]);
  const hasQuery = query.trim().length > 0;
  const noMatch = !loading && hasQuery && filtered.length === 0;

  useEffect(() => {
    if (!hasQuery || advancedItems.length === 0) {
      return;
    }
    setActiveKeys((current) => (current.includes('advanced') ? current : [...current, 'advanced']));
  }, [hasQuery, advancedItems.length]);

  const setScopeEnabled = (scope: string, checked: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      if (checked) {
        next.add(scope);
      } else {
        next.delete(scope);
      }
      return next;
    });
  };

  const restoreDefault = () => {
    setSelected(new Set(catalog.filter((item) => item.core).map((item) => item.scope)));
  };

  const save = async () => {
    try {
      setSaving(true);
      await api.setAppPolicy(props.app.id, [...selected].sort());
      Toast.success(copy.apps.saved);
      await props.onSaved();
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setSaving(false);
    }
  };

  const goAccounts = () => {
    props.onClose();
    props.onGoAccounts();
  };

  return (
    <SideSheet
      className="scope-policy-drawer"
      title={copy.apps.drawerTitle(props.app.label)}
      visible={props.visible}
      onCancel={props.onClose}
      size="medium"
      closeOnEsc={!saving}
      maskClosable={!saving}
      footer={(
        <div className="scope-policy-drawer-footer">
          <Button disabled={loading || saving || catalog.length === 0} onClick={restoreDefault}>
            {copy.apps.restoreDefault}
          </Button>
          <Space wrap>
            <Button disabled={saving} onClick={props.onClose}>{copy.common.cancel}</Button>
            <Button
              theme="solid"
              loading={saving}
              disabled={!dirty || overLimit || loading || saving}
              onClick={() => void save()}
            >
              {copy.apps.save}
            </Button>
          </Space>
        </div>
      )}
    >
      <Space vertical align="start" spacing="loose" className="full-width">
        <Banner
          type="info"
          fullMode={false}
          closeIcon={null}
          title={copy.apps.takeEffectTitle}
          description={copy.apps.takeEffectHint}
        >
          {dirty ? (
            <Popconfirm
              title={copy.apps.discardTitle}
              content={copy.apps.discardHint}
              okText={copy.apps.discardConfirm}
              cancelText={copy.common.cancel}
              onConfirm={goAccounts}
            >
              <Button theme="light" disabled={saving}>{copy.apps.goReauth}</Button>
            </Popconfirm>
          ) : (
            <Button theme="light" disabled={saving} onClick={goAccounts}>
              {copy.apps.goReauth}
            </Button>
          )}
        </Banner>
        {overLimit && (
          <Banner
            type="danger"
            fullMode={false}
            closeIcon={null}
            description={copy.apps.overLimit(MAX_SINGLE_AUTH_SCOPES)}
          />
        )}
        <Input
          value={query}
          onChange={setQuery}
          placeholder={copy.apps.searchPlaceholder}
          prefix={<IconSearch />}
          showClear
          aria-label={copy.apps.searchPlaceholder}
          style={{ width: '100%' }}
        />
        <Text>{copy.apps.selectedCount(selected.size, MAX_SINGLE_AUTH_SCOPES)}</Text>
        <Spin spinning={loading} style={{ width: '100%' }}>
          {noMatch ? (
            <Empty title={copy.apps.noMatch} />
          ) : (
            <Collapse
              activeKey={activeKeys}
              onChange={(keys) => setActiveKeys(collapseKeys(keys))}
            >
              {coreGroups.length > 0 && (
                <Collapse.Panel header={copy.apps.coreSection} itemKey="core">
                  <Paragraph type="tertiary">{copy.apps.coreSectionHint}</Paragraph>
                  <ModuleGroupList
                    groups={coreGroups}
                    selected={selected}
                    onCheckedChange={setScopeEnabled}
                  />
                </Collapse.Panel>
              )}
              {advancedGroups.length > 0 && (
                <Collapse.Panel header={copy.apps.advancedSection} itemKey="advanced">
                  <Paragraph type="tertiary">{copy.apps.advancedSectionHint}</Paragraph>
                  <ModuleGroupList
                    groups={advancedGroups}
                    selected={selected}
                    onCheckedChange={setScopeEnabled}
                  />
                </Collapse.Panel>
              )}
            </Collapse>
          )}
        </Spin>
      </Space>
    </SideSheet>
  );
}
