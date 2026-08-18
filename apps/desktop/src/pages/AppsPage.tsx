import { useMemo, useState } from 'react';
import { Banner, Button, Card, Empty, Space, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { IconApps, IconPlus, IconRefresh } from '@douyinfe/semi-icons';
import { api } from '../api';
import { PageHeader } from '../components/PageHeader';
import { ScopePolicyDrawer } from '../components/ScopePolicyDrawer';
import { copy } from '../copy';
import type { AppRecord, Snapshot } from '../types';
import { groupScopes, normalizeError } from '../utils';

const { Title, Text, Paragraph } = Typography;

export interface AppsPageProps {
  data: Snapshot;
  onCreate: () => void;
  onImport: () => void;
  onReload: () => Promise<void>;
  onGoAccounts: () => void;
}

export function AppsPage(props: AppsPageProps) {
  return (
    <section>
      <PageHeader
        title={copy.apps.title}
        description={copy.apps.description}
        detailHeader={copy.apps.detailHeader}
        detail={copy.apps.detail}
        action={<Button icon={<IconPlus />} theme="solid" onClick={props.onImport}>{copy.apps.import}</Button>}
      />
      <Space vertical align="start" spacing="loose" className="full-width">
        <Button
          onClick={props.onCreate}
        >
          {copy.apps.create}
        </Button>
        {props.data.apps.length === 0 ? (
          <Empty title={copy.apps.emptyTitle} description={copy.apps.emptyDescription} />
        ) : (
          props.data.apps.map((app) => (
            <AppPolicyCard
              key={app.id}
              app={app}
              onReload={props.onReload}
              onGoAccounts={props.onGoAccounts}
            />
          ))
        )}
      </Space>
    </section>
  );
}

interface AppPolicyCardProps {
  app: AppRecord;
  onReload: () => Promise<void>;
  onGoAccounts: () => void;
}

function AppPolicyCard(props: AppPolicyCardProps) {
  const [editing, setEditing] = useState(false);
  const groups = useMemo(() => groupScopes(props.app.availableScopes), [props.app.availableScopes]);
  const enabled = useMemo(() => new Set(props.app.policyScopes), [props.app.policyScopes]);
  const newCount = props.app.availableScopes.filter((scope) => !props.app.policyScopes.includes(scope)).length;

  return (
    <Card className="app-card full-width">
      <div className="card-title-row">
        <div className="app-icon"><IconApps /></div>
        <div className="grow">
          <Title heading={4}>{props.app.label}</Title>
          <Text type="tertiary">{props.app.appId} · {props.app.brand}</Text>
        </div>
        <Tag color="blue">{copy.apps.enabledCount(props.app.policyScopes.length, props.app.availableScopes.length)}</Tag>
      </div>
      {newCount > 0 && (
        <Banner
          type="info"
          className="inline-banner"
          closeIcon={null}
          description={(
            <Space>
              <span>{copy.apps.unusedScopes(newCount)}</span>
              <Button theme="borderless" onClick={() => setEditing(true)}>
                {copy.apps.editScopes}
              </Button>
            </Space>
          )}
        />
      )}
      <div className="policy-actions">
        <Button theme="solid" onClick={() => setEditing(true)}>
          {copy.apps.editScopes}
        </Button>
        <Button
          theme="borderless"
          icon={<IconRefresh />}
          onClick={async () => {
            try {
              await api.refreshAppScopes(props.app.id);
              Toast.success(copy.apps.refreshed);
              await props.onReload();
            } catch (error) {
              Toast.error(normalizeError(error));
            }
          }}
        >
          {copy.apps.refresh}
        </Button>
      </div>
      <details className="scope-details">
        <summary>{copy.apps.scopeSummary}</summary>
        <Paragraph type="tertiary">
          {copy.apps.scopeHint}
        </Paragraph>
        {Object.entries(groups).map(([module, scopes]) => (
          <div className="scope-group" key={module}>
            <div className="scope-group-title">{module}</div>
            {scopes.map((scope) => (
              <div className="scope-policy-row" key={scope}>
                <code>{scope}</code>
                <Tag color={enabled.has(scope) ? 'green' : 'grey'}>
                  {enabled.has(scope) ? copy.apps.enabled : copy.apps.disabled}
                </Tag>
              </div>
            ))}
          </div>
        ))}
      </details>
      <ScopePolicyDrawer
        app={props.app}
        visible={editing}
        onClose={() => setEditing(false)}
        onSaved={props.onReload}
        onGoAccounts={props.onGoAccounts}
      />
    </Card>
  );
}
