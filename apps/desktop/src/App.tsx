import { useEffect, useRef, useState } from 'react';
import {
  Banner,
  Button,
  Collapse,
  Modal,
  Select,
  Spin,
  Tag,
  Toast,
  Typography,
} from '@douyinfe/semi-ui';
import {
  IconApps,
  IconDesktop,
  IconSetting,
  IconUser,
} from '@douyinfe/semi-icons';
import { openUrl } from '@tauri-apps/plugin-opener';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { listen } from '@tauri-apps/api/event';
import { api } from './api';
import appIcon from './assets/app-icon.svg';
import {
  type AuthorizationMode,
  type AuthorizationPollPhase,
  authBytesToObjectUrl,
  authorizationStatusText,
  formatRemainingTime,
  isAuthorizationExpired,
  isDuplicateAccountError,
  maskAppId,
  nextAuthorizationMode,
} from './auth-presentation';
import { NavButton } from './components/NavButton';
import { copy } from './copy';
import { ImportAppModal } from './modals/ImportAppModal';
import { ImportExistingAccountModal, type DiscoveryStatus } from './modals/ImportExistingAccountModal';
import { OfficialAppCreationModal } from './modals/OfficialAppCreationModal';
import { AccountsPage } from './pages/AccountsPage';
import { AppsPage } from './pages/AppsPage';
import { SettingsPage } from './pages/SettingsPage';
import { SystemPage } from './pages/SystemPage';
import type {
  AccountHealth,
  AuthFlowStart,
  DiagnosticReport,
  ExistingCliCandidate,
  Snapshot,
} from './types';
import { normalizeError } from './utils';

const { Text, Paragraph } = Typography;
type Page = 'accounts' | 'apps' | 'system' | 'settings';
type AuthorizationOrigin =
  | { kind: 'new-account'; appRef: string }
  | { kind: 'reauthorization'; accountId: string };

function App() {
  const [page, setPage] = useState<Page>('accounts');
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [importVisible, setImportVisible] = useState(false);
  const [appCreationVisible, setAppCreationVisible] = useState(false);
  const [localImportVisible, setLocalImportVisible] = useState(false);
  const [addAccountVisible, setAddAccountVisible] = useState(false);
  const [selectedApp, setSelectedApp] = useState<string>('');
  const [auth, setAuth] = useState<AuthFlowStart | null>(null);
  const [authBusy, setAuthBusy] = useState(false);
  const [authMode, setAuthMode] = useState<AuthorizationMode>('qr');
  const [authPollPhase, setAuthPollPhase] = useState<AuthorizationPollPhase>('waiting');
  const [authQrUrl, setAuthQrUrl] = useState<string | null>(null);
  const [authRemaining, setAuthRemaining] = useState(0);
  const [qrBusy, setQrBusy] = useState(false);
  const [authOrigin, setAuthOrigin] = useState<AuthorizationOrigin | null>(null);
  const [authRestartRequired, setAuthRestartRequired] = useState(false);
  const [diagnostics, setDiagnostics] = useState<DiagnosticReport | null>(null);
  const [diagnosing, setDiagnosing] = useState(false);
  const [migrationCandidates, setMigrationCandidates] = useState<ExistingCliCandidate[]>([]);
  const [migrationDiscoveryStatus, setMigrationDiscoveryStatus] = useState<DiscoveryStatus>('idle');
  const [migrationDiscoveryError, setMigrationDiscoveryError] = useState('');
  const authPollingFlowRef = useRef<string | null>(null);
  const authUnavailable = isAuthorizationExpired(authRemaining) || authRestartRequired;

  const reload = async () => {
    try {
      setSnapshot(await api.snapshot());
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setLoading(false);
    }
  };

  const refreshDiagnostics = async () => {
    try {
      setDiagnosing(true);
      setDiagnostics(await api.diagnose());
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setDiagnosing(false);
    }
  };

  const refreshMigrationCandidates = async () => {
    setMigrationDiscoveryStatus('scanning');
    setMigrationDiscoveryError('');
    try {
      setMigrationCandidates(await api.discoverExistingConfigs());
      setMigrationDiscoveryStatus('success');
    } catch (error) {
      // Discovery is advisory. An explicit path can still be inspected from
      // the import dialog if a default location is invalid.
      setMigrationCandidates([]);
      setMigrationDiscoveryError(normalizeError(error));
      setMigrationDiscoveryStatus('error');
    }
  };

  useEffect(() => {
    void reload();
    void refreshMigrationCandidates();
    let unlistenHealth: (() => void) | undefined;
    let unlistenDegraded: (() => void) | undefined;
    let unlistenCliff: (() => void) | undefined;
    void listen('lpc://health-updated', () => void reload()).then((dispose) => {
      unlistenHealth = dispose;
    });
    void listen<{ accountName: string; health: AccountHealth; detail: string }>(
      'lpc://health-degraded',
      (event) => {
        Toast.error(copy.accounts.healthDegraded(event.payload.accountName, event.payload.detail));
        void reload();
      },
    ).then((dispose) => {
      unlistenDegraded = dispose;
    });
    void listen<{ from: number; to: number; detail: string }>(
      'lpc://keychain-cliff',
      (event) => {
        Toast.error(copy.accounts.keychainCliff(event.payload.from, event.payload.to));
        void reload();
      },
    ).then((dispose) => {
      unlistenCliff = dispose;
    });
    return () => {
      unlistenHealth?.();
      unlistenDegraded?.();
      unlistenCliff?.();
    };
  }, []);

  useEffect(() => {
    setAuthMode('qr');
  }, [auth?.flowId]);

  useEffect(() => {
    if (page === 'system') {
      void refreshDiagnostics();
    }
  }, [page]);

  useEffect(() => {
    setAuthQrUrl(null);
    if (!auth?.qrCodePng?.length) return;

    const objectUrl = authBytesToObjectUrl(auth.qrCodePng);
    setAuthQrUrl(objectUrl);
    return () => URL.revokeObjectURL(objectUrl);
  }, [auth?.qrCodePng]);

  useEffect(() => {
    if (!auth || authRestartRequired) {
      setAuthRemaining(0);
      return;
    }

    const expiresAt = Date.now() + auth.expiresIn * 1000;
    const updateRemaining = () => {
      setAuthRemaining(Math.max(0, Math.ceil((expiresAt - Date.now()) / 1000)));
    };
    updateRemaining();
    const timer = window.setInterval(updateRemaining, 1000);
    return () => window.clearInterval(timer);
  }, [auth?.expiresIn, auth?.flowId, auth?.verificationUrl, authRestartRequired]);

  const switchAccount = async (accountId: string) => {
    if (!snapshot) return;
    const old = snapshot.accounts.find((item) => item.active);
    try {
      const next = await api.switchAccount(accountId);
      setSnapshot(next);
      const oldRunning = old?.runningCommands ?? 0;
      Toast.success(
        oldRunning > 0
          ? copy.accounts.switchedWithRunning(oldRunning)
          : copy.accounts.switched,
      );
    } catch (error) {
      Toast.error(normalizeError(error));
    }
  };

  const startLogin = async (appRef: string) => {
    try {
      setAuthBusy(true);
      const flow = await api.beginAccountLogin(appRef);
      setAuthOrigin({ kind: 'new-account', appRef });
      setAuthPollPhase('waiting');
      setAuthRestartRequired(false);
      setAuthRemaining(flow.expiresIn);
      setAuth(flow);
      setAddAccountVisible(false);
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setAuthBusy(false);
    }
  };

  const startReauth = async (accountId: string) => {
    try {
      setAuthBusy(true);
      const flow = await api.beginReauthorization(accountId);
      setAuthOrigin({ kind: 'reauthorization', accountId });
      setAuthPollPhase('waiting');
      setAuthRestartRequired(false);
      setAuthRemaining(flow.expiresIn);
      setAuth(flow);
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setAuthBusy(false);
    }
  };

  const pollAuthorization = async (flowId: string) => {
    if (authPollingFlowRef.current !== flowId) return;
    try {
      setAuthPollPhase('checking');
      const progress = await api.completeAuthorization(flowId);
      if (authPollingFlowRef.current !== flowId) return;
      if (progress.complete) {
        authPollingFlowRef.current = null;
        Toast.success(copy.auth.completed);
        setAuth(null);
        setAuthOrigin(null);
        setAuthRestartRequired(false);
        await reload();
      } else {
        setAuthPollPhase('waiting');
        window.setTimeout(() => void pollAuthorization(flowId), 500);
      }
    } catch (error) {
      if (authPollingFlowRef.current !== flowId) return;
      authPollingFlowRef.current = null;
      if (isDuplicateAccountError(error)) {
        try {
          await api.cancelAuthorization(flowId);
        } catch {
          // Completion already removes a rejected flow; cancellation is best-effort cleanup.
        }
        setAuth(null);
        setAuthOrigin(null);
        setAuthRestartRequired(false);
        await reload();
        Toast.info(copy.auth.duplicate);
      } else {
        setAuthRestartRequired(true);
        setAuthRemaining(0);
        Toast.error(normalizeError(error));
      }
    }
  };

  useEffect(() => {
    if (!auth || authRestartRequired) return;
    authPollingFlowRef.current = auth.flowId;
    void pollAuthorization(auth.flowId);
    return () => {
      if (authPollingFlowRef.current === auth.flowId) {
        authPollingFlowRef.current = null;
      }
    };
  }, [auth?.flowId, authRestartRequired]);

  const cancelAuth = async () => {
    const flowId = auth?.flowId;
    authPollingFlowRef.current = null;
    if (flowId) {
      try {
        setAuthBusy(true);
        await api.cancelAuthorization(flowId);
      } catch {
        // The flow may already have expired. Local UI can still close safely.
      }
    }
    setAuth(null);
    setAuthOrigin(null);
    setAuthRestartRequired(false);
    setAuthMode('qr');
    setAuthPollPhase('waiting');
    setAuthQrUrl(null);
    setAuthRemaining(0);
    setQrBusy(false);
    setAuthBusy(false);
  };

  const retryAuthorizationQr = async () => {
    const flowId = auth?.flowId;
    if (!flowId || authUnavailable) return;
    try {
      setQrBusy(true);
      const qrCodePng = await api.renderAuthorizationQr(flowId);
      setAuth((current) => current?.flowId === flowId ? { ...current, qrCodePng } : current);
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setQrBusy(false);
    }
  };

  const copyAuthorizationUrl = async () => {
    if (!auth || authUnavailable) return;
    try {
      await writeText(auth.verificationUrl);
      Toast.success(copy.auth.copied);
    } catch (error) {
      Toast.error(normalizeError(error));
    }
  };

  const openAuthorizationUrl = async () => {
    if (!auth || authUnavailable) return;
    try {
      await openUrl(auth.verificationUrl);
    } catch (error) {
      Toast.error(normalizeError(error));
    }
  };

  const restartAuthorization = async () => {
    if (!authOrigin) return;
    const oldFlowId = auth?.flowId;
    try {
      setAuthBusy(true);
      if (oldFlowId) {
        try {
          await api.cancelAuthorization(oldFlowId);
        } catch {
          // Expiry and failed completion may already have removed this flow.
        }
      }

      const flow = authOrigin.kind === 'new-account'
        ? await api.beginAccountLogin(authOrigin.appRef)
        : await api.beginReauthorization(authOrigin.accountId);
      setAuthMode('qr');
      setAuthPollPhase('waiting');
      setAuthRestartRequired(false);
      setAuthRemaining(flow.expiresIn);
      setAuth(flow);
    } catch (error) {
      setAuthRestartRequired(true);
      setAuthRemaining(0);
      Toast.error(normalizeError(error));
    } finally {
      setAuthBusy(false);
    }
  };

  const handleAuthorizationTabKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    const nextMode = nextAuthorizationMode(authMode, event.key);
    if (!nextMode) return;

    event.preventDefault();
    setAuthMode(nextMode);
    document.getElementById(`oauth-authorization-${nextMode}-tab`)?.focus();
  };

  if (loading) {
    return (
      <div className="center-screen">
        <Spin size="large" />
      </div>
    );
  }

  const pendingMigrationCount = migrationCandidates.filter((candidate) => !candidate.alreadyImported).length;

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand-block">
          <img className="brand-icon" src={appIcon} alt="" aria-hidden="true" />
          <div className="brand-copy">
            <div className="brand-title">larkswitch</div>
          </div>
        </div>
        <nav className="nav-list">
          <NavButton active={page === 'accounts'} icon={<IconUser />} onClick={() => setPage('accounts')}>
            {copy.nav.accounts}
          </NavButton>
          <NavButton active={page === 'apps'} icon={<IconApps />} onClick={() => setPage('apps')}>
            {copy.nav.apps}
          </NavButton>
          <NavButton
            active={page === 'system'}
            icon={<IconDesktop />}
            onClick={() => {
              if (page !== 'system') {
                setDiagnosing(true);
              }
              setPage('system');
            }}
          >
            {copy.nav.system}
          </NavButton>
          <NavButton active={page === 'settings'} icon={<IconSetting />} onClick={() => setPage('settings')}>
            {copy.nav.settings}
          </NavButton>
        </nav>
      </aside>

      <main className="main-content">
        {snapshot && !snapshot.state.managedCliPath && (
          <div className="setup-callout">
            <div>
              <Text strong>{copy.setup.title}</Text>
              <div><Text type="tertiary">{copy.setup.description}</Text></div>
              <Collapse>
                <Collapse.Panel header={copy.setup.detailHeader} itemKey="setup">
                  <Text type="tertiary">{copy.setup.detail}</Text>
                </Collapse.Panel>
              </Collapse>
            </div>
            <Button
              theme="solid"
              onClick={async () => {
                try {
                  setLoading(true);
                  await api.installRuntime(snapshot.settings.recommendedCliVersion);
                  Toast.success(copy.setup.done);
                  await reload();
                  await refreshMigrationCandidates();
                } catch (error) {
                  Toast.error(normalizeError(error));
                  setLoading(false);
                }
              }}
            >{copy.setup.action}</Button>
          </div>
        )}
        {snapshot?.state.managedCliPath
          && pendingMigrationCount > 0 && (
          <div className="setup-callout">
            <div>
              <Text strong>
                {copy.migration.title(pendingMigrationCount)}
              </Text>
              <div>
                <Text type="tertiary">{copy.migration.description}</Text>
              </div>
              <Collapse>
                <Collapse.Panel header={copy.migration.detailHeader} itemKey="migration">
                  <Text type="tertiary">{copy.migration.detail}</Text>
                </Collapse.Panel>
              </Collapse>
            </div>
            <Button theme="solid" onClick={() => setLocalImportVisible(true)}>{copy.migration.action}</Button>
          </div>
        )}
        {page === 'accounts' && snapshot && (
          <AccountsPage
            data={snapshot}
            onSwitch={switchAccount}
            onAdd={() => {
              setSelectedApp(snapshot.apps[0]?.id ?? '');
              setAddAccountVisible(true);
            }}
            onImportLocal={() => setLocalImportVisible(true)}
            onReauth={startReauth}
            onReload={reload}
          />
        )}
        {page === 'apps' && snapshot && (
          <AppsPage
            data={snapshot}
            onCreate={() => setAppCreationVisible(true)}
            onImport={() => setImportVisible(true)}
            onReload={reload}
            onGoAccounts={() => setPage('accounts')}
          />
        )}
        {page === 'system' && snapshot && (
          <SystemPage
            data={snapshot}
            diagnostics={diagnostics}
            diagnosing={diagnosing}
            onDiagnostics={refreshDiagnostics}
            onReload={reload}
          />
        )}
        {page === 'settings' && snapshot && <SettingsPage data={snapshot} onReload={reload} />}
      </main>

      <ImportAppModal visible={importVisible} onClose={() => setImportVisible(false)} onDone={reload} />
      <OfficialAppCreationModal
        visible={appCreationVisible}
        onClose={() => setAppCreationVisible(false)}
        onDone={reload}
      />
      <ImportExistingAccountModal
        visible={localImportVisible}
        initialCandidates={migrationCandidates}
        initialDiscoveryStatus={migrationDiscoveryStatus}
        initialDiscoveryError={migrationDiscoveryError}
        onClose={() => setLocalImportVisible(false)}
        onDone={async () => {
          await reload();
          await refreshMigrationCandidates();
        }}
      />

      <Modal
        title={copy.addAccount.title}
        visible={addAccountVisible}
        onCancel={() => setAddAccountVisible(false)}
        className="lpc-modal"
        modalContentClass="lpc-modal-content"
        footer={(
          <>
            <Button disabled={authBusy} onClick={() => setAddAccountVisible(false)}>{copy.common.cancel}</Button>
            <Button
              theme="solid"
              loading={authBusy}
              disabled={!selectedApp}
              onClick={() => void startLogin(selectedApp)}
            >
              {copy.addAccount.start}
            </Button>
          </>
        )}
        width={500}
      >
        <div className="modal-stack">
          <Paragraph type="tertiary">
            {copy.addAccount.description}
          </Paragraph>
          <label>
            <span id="add-account-app-label">授权 App</span>
            <Select
              aria-labelledby="add-account-app-label"
              className="full-width"
              inputProps={{ 'aria-label': '授权 App' }}
              value={selectedApp}
              onChange={(value) => setSelectedApp(String(value))}
              optionList={(snapshot?.apps ?? []).map((app) => {
                const primaryLabel = copy.addAccount.option(
                  app.brand === 'feishu' ? copy.addAccount.brandFeishu : copy.addAccount.brandLark,
                  maskAppId(app.appId),
                );
                return {
                  value: app.id,
                  primaryLabel,
                  secondaryLabel: app.label,
                  label: (
                    <div className="app-option-label">
                      <span>{primaryLabel}</span>
                      <span className="app-option-secondary">{app.label}</span>
                    </div>
                  ),
                };
              })}
              renderSelectedItem={(option: Record<string, unknown>) => (
                <span className="app-selected-label">
                  <span className="app-selected-primary">{String(option.primaryLabel)}</span>
                  <span className="app-selected-secondary">{String(option.secondaryLabel)}</span>
                </span>
              )}
              placeholder={copy.addAccount.placeholder}
            />
          </label>
        </div>
      </Modal>

      <Modal
        title={copy.auth.title}
        visible={Boolean(auth)}
        onCancel={() => void cancelAuth()}
        className="lpc-modal"
        modalContentClass="lpc-modal-content"
        footer={authUnavailable ? (
          <Button
            theme="solid"
            loading={authBusy}
            disabled={!authOrigin}
            onClick={() => void restartAuthorization()}
          >
            {copy.auth.restart}
          </Button>
        ) : null}
        width={620}
        closable={!authBusy}
        closeOnEsc={!authBusy}
        maskClosable={!authBusy}
      >
        {auth && (
          <div className="auth-panel">
            <Banner
              type="info"
              description={copy.auth.banner}
            />
            <div className="auth-summary">
              <Text strong>{copy.auth.capabilities}</Text>
              <div className="tag-row">
                {auth.batch.modules.map((module) => (
                  <Tag key={module}>{module}</Tag>
                ))}
              </div>
              <Text type="tertiary">
                {copy.auth.willRequestListed}
              </Text>
            </div>
            <div className="auth-expiry" aria-live="polite">
              <Text type="tertiary">{copy.auth.remaining}</Text>
              <Text strong>{formatRemainingTime(authRemaining)}</Text>
            </div>
            {!authUnavailable && (
              <div className="auth-poll-status" aria-live="polite">
                <Spin size="small" />
                <Text>{authorizationStatusText(authPollPhase)}</Text>
              </div>
            )}
            {authUnavailable && (
              <Banner type="warning" description={copy.auth.expired} />
            )}
            <div className="auth-mode-tabs" role="tablist" aria-label={copy.auth.modeAria}>
              <button
                id="oauth-authorization-qr-tab"
                type="button"
                role="tab"
                aria-controls="oauth-authorization-qr-panel"
                aria-selected={authMode === 'qr'}
                className={authMode === 'qr' ? 'active' : ''}
                onClick={() => setAuthMode('qr')}
                onKeyDown={handleAuthorizationTabKeyDown}
                tabIndex={authMode === 'qr' ? 0 : -1}
              >
                {copy.auth.qr}
              </button>
              <button
                id="oauth-authorization-browser-tab"
                type="button"
                role="tab"
                aria-controls="oauth-authorization-browser-panel"
                aria-selected={authMode === 'browser'}
                className={authMode === 'browser' ? 'active' : ''}
                onClick={() => setAuthMode('browser')}
                onKeyDown={handleAuthorizationTabKeyDown}
                tabIndex={authMode === 'browser' ? 0 : -1}
              >
                {copy.auth.browser}
              </button>
            </div>
            {authMode === 'qr' ? (
              <div
                id="oauth-authorization-qr-panel"
                role="tabpanel"
                aria-labelledby="oauth-authorization-qr-tab"
                className="auth-mode-panel qr-mode-panel"
              >
                <Text strong>{copy.auth.qrHint}</Text>
                {authQrUrl && !authUnavailable ? (
                  <img className="auth-qr-image" src={authQrUrl} alt={copy.auth.qrAlt} />
                ) : (
                  <div className="auth-qr-fallback">
                    <Text type="tertiary">
                      {authUnavailable
                        ? copy.auth.qrExpired
                        : copy.auth.qrUnavailable}
                    </Text>
                    {!authUnavailable && (
                      <Button loading={qrBusy} onClick={() => void retryAuthorizationQr()}>
                        {copy.auth.retryQr}
                      </Button>
                    )}
                  </div>
                )}
              </div>
            ) : (
              <div
                id="oauth-authorization-browser-panel"
                role="tabpanel"
                aria-labelledby="oauth-authorization-browser-tab"
                className="auth-mode-panel"
              >
                <Banner
                  type="warning"
                  description={copy.auth.browserWarning}
                />
                <label className="auth-url-field">
                  {copy.auth.urlLabel}
                  <textarea
                    aria-label={copy.auth.urlLabel}
                    disabled={authUnavailable}
                    readOnly
                    rows={3}
                    value={auth.verificationUrl}
                  />
                </label>
                <div className="auth-url-actions">
                  <Button disabled={authUnavailable} onClick={() => void copyAuthorizationUrl()}>
                    {copy.auth.copyUrl}
                  </Button>
                  <Button disabled={authUnavailable} theme="solid" onClick={() => void openAuthorizationUrl()}>
                    {copy.auth.openBrowser}
                  </Button>
                </div>
              </div>
            )}
          </div>
        )}
      </Modal>
    </div>
  );
}

export default App;
