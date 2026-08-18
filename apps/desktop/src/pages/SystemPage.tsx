import { useState } from 'react';
import { Button, Empty, Popconfirm, Space, Spin, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { IconDesktop, IconKey } from '@douyinfe/semi-icons';
import { api } from '../api';
import { PageHeader } from '../components/PageHeader';
import { StatusCard } from '../components/StatusCard';
import { copy } from '../copy';
import type { DiagnosticReport, Snapshot } from '../types';
import { normalizeError } from '../utils';

const { Text } = Typography;

export interface SystemPageProps {
  data: Snapshot;
  diagnostics: DiagnosticReport | null;
  diagnosing: boolean;
  onDiagnostics: () => Promise<void>;
  onReload: () => Promise<void>;
}

export function SystemPage(props: SystemPageProps) {
  const [installing, setInstalling] = useState(false);
  const [rollingBack, setRollingBack] = useState(false);
  const cliReady = Boolean(props.data.state.managedCliPath);
  const pathTakeoverEnabled = props.data.settings.pathTakeoverEnabled;
  const locked = installing || rollingBack || props.diagnosing;

  const runInstall = async () => {
    try {
      setInstalling(true);
      await api.installRuntime(props.data.settings.recommendedCliVersion);
      Toast.success(copy.system.installed);
      await props.onReload();
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setInstalling(false);
    }
  };

  const runRollback = async () => {
    try {
      setRollingBack(true);
      await api.rollbackRuntime();
      Toast.success(copy.system.rolledBack);
      await props.onReload();
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setRollingBack(false);
    }
  };

  const runDiagnose = async () => {
    try {
      await props.onDiagnostics();
    } catch (error) {
      Toast.error(normalizeError(error));
    }
  };

  return (
    <section>
      <PageHeader
        title={copy.system.title}
        description={copy.system.description}
        detailHeader={copy.system.detailHeader}
        detail={copy.system.detail}
      />
      <div className="status-grid">
        <StatusCard
          icon={<IconDesktop />}
          title={copy.system.cliTitle}
          value={props.data.state.managedCliVersion ?? copy.system.cliMissing}
          detail={props.data.state.managedCliPath ?? copy.system.cliUnmanaged}
          ok={cliReady}
          extra={cliReady ? (
            <Popconfirm
              title={copy.system.rollbackTitle}
              content={copy.system.rollbackHint}
              onConfirm={() => runRollback()}
            >
              <Button
                theme="borderless"
                disabled={locked}
                loading={rollingBack}
              >
                {copy.system.rollback}
              </Button>
            </Popconfirm>
          ) : undefined}
        />
        <StatusCard
          icon={<IconKey />}
          title={copy.system.routeTitle}
          value={props.data.state.activeAccountId ? copy.system.routeSelected : copy.system.routeMissing}
          detail={props.data.state.activeAccountId ? copy.system.routeSelectedDetail : copy.system.routeMissingDetail}
          ok={Boolean(props.data.state.activeAccountId)}
        />
      </div>
      <Space vertical align="start" spacing="medium" className="full-width">
        <div className="settings-list">
          <div className="settings-row settings-row-static">
            <div className="settings-copy">
              <Text strong>{copy.settings.pathTakeover}</Text>
              <Text type="tertiary">{copy.system.pathTakeoverHint}</Text>
            </div>
            <Tag color={pathTakeoverEnabled ? 'green' : 'grey'}>
              {pathTakeoverEnabled ? copy.system.pathTakeoverOn : copy.system.pathTakeoverOff}
            </Tag>
          </div>
        </div>
        <Space spacing="medium" wrap>
          <Button
            theme={cliReady ? 'light' : 'solid'}
            loading={installing}
            disabled={locked}
            onClick={() => void runInstall()}
          >
            {copy.system.install}
          </Button>
          <Button
            loading={props.diagnosing}
            disabled={locked}
            onClick={() => void runDiagnose()}
          >
            {copy.system.diagnose}
          </Button>
        </Space>
        <Spin spinning={props.diagnosing} wrapperClassName="full-width">
          {props.diagnostics ? (
            <div className="diagnostic-list">
              {props.diagnostics.checks.map((check) => (
                <div className="diagnostic-row" key={check.id}>
                  <Tag color={check.status === 'pass' ? 'green' : check.status === 'warn' ? 'orange' : 'red'}>
                    {check.status}
                  </Tag>
                  <div>
                    <Text strong>{check.summary}</Text>
                    <div><Text type="tertiary">{check.detail || copy.system.noDetail}</Text></div>
                  </div>
                </div>
              ))}
            </div>
          ) : props.diagnosing ? (
            <div className="diagnostic-list diagnostic-empty" />
          ) : (
            <div className="diagnostic-list diagnostic-empty">
              <Empty
                title={copy.system.diagnoseEmptyTitle}
                description={copy.system.diagnoseEmptyDescription}
              />
            </div>
          )}
        </Spin>
      </Space>
    </section>
  );
}
