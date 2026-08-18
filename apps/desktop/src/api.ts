import { invoke } from '@tauri-apps/api/core';
import type {
  AccountRecord,
  AppRecord,
  AppCreationProgress,
  AppCreationStart,
  AuthFlowStart,
  AuthProgress,
  Brand,
  DiagnosticReport,
  ExistingAccountImport,
  ExistingCliCandidate,
  ScopeInfo,
  RuntimeIdentity,
  Snapshot,
} from './types';

export const api = {
  snapshot: () => invoke<Snapshot>('snapshot'),
  switchAccount: (accountId: string) => invoke<Snapshot>('switch_account', { accountId }),
  installRuntime: (version: string) => invoke<string>('install_runtime', { version }),
  rollbackRuntime: () => invoke<string>('rollback_runtime'),
  installPathTakeover: () => invoke('install_path_takeover'),
  removePathTakeover: () => invoke('remove_path_takeover'),
  autostartStatus: () => invoke<boolean>('autostart_status'),
  setAutostart: (enabled: boolean) => invoke<boolean>('set_autostart', { enabled }),
  importExistingApp: (request: {
    label: string;
    appId: string;
    appSecret: string;
    brand: Brand;
  }) => invoke<AppRecord>('import_existing_app', { request }),
  discoverExistingConfigs: () =>
    invoke<ExistingCliCandidate[]>('discover_existing_configs'),
  inspectExistingConfig: (configDir: string) =>
    invoke<ExistingCliCandidate>('inspect_existing_config', { configDir }),
  importExistingAccountConfig: (label: string, configDir: string) =>
    invoke<ExistingAccountImport>('import_existing_account_config', { label, configDir }),
  beginOfficialAppCreation: (label: string, brand: Brand) =>
    invoke<AppCreationStart>('begin_official_app_creation', { label, brand }),
  pollOfficialAppCreation: (flowId: string) =>
    invoke<AppCreationProgress>('poll_official_app_creation', { flowId }),
  cancelOfficialAppCreation: (flowId: string) =>
    invoke<void>('cancel_official_app_creation', { flowId }),
  refreshAppScopes: (appRef: string) => invoke<AppRecord>('refresh_app_scopes', { appRef }),
  setAppPolicy: (appRef: string, scopes: string[]) =>
    invoke<AppRecord>('set_app_policy', { request: { appRef, scopes } }),
  scopeCatalog: (appRef: string) => invoke<ScopeInfo[]>('scope_catalog', { appRef }),
  setAccountAlias: (accountId: string, alias: string) =>
    invoke<AccountRecord>('set_account_alias', { accountId, alias }),
  clearAccountAlias: (accountId: string) =>
    invoke<AccountRecord>('clear_account_alias', { accountId }),
  beginAccountLogin: (appRef: string) => invoke<AuthFlowStart>('begin_account_login', { appRef }),
  beginReauthorization: (accountId: string) =>
    invoke<AuthFlowStart>('begin_reauthorization', { accountId }),
  renderAuthorizationQr: (flowId: string) =>
    invoke<number[]>('render_authorization_qr', { flowId }),
  completeAuthorization: (flowId: string) =>
    invoke<AuthProgress>('complete_authorization', { flowId }),
  cancelAuthorization: (flowId: string) => invoke<void>('cancel_authorization', { flowId }),
  checkAccount: (accountId: string) => invoke('check_account', { accountId }),
  removeAccount: (accountId: string) => invoke<void>('remove_account', { accountId }),
  diagnose: () => invoke<DiagnosticReport>('diagnose'),
  runtimeIdentity: () => invoke<RuntimeIdentity>('runtime_identity'),
};
