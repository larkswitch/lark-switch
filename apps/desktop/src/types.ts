export type Brand = 'feishu' | 'lark';
export type AccountHealth =
  | 'unknown'
  | 'ready'
  | 'refreshable'
  | 'reauth_required'
  | 'temporary_failure'
  | 'cli_failure';
export type CredentialOrigin = 'managed' | 'external_shared';

export interface ScopeInfo {
  scope: string;
  core: boolean;
  reason?: string | null;
}

export interface AppRecord {
  id: string;
  appId: string;
  label: string;
  brand: Brand;
  baseConfigPath: string;
  availableScopes: string[];
  policyScopes: string[];
  scopesObservedAt?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface AppCreationStart {
  flowId: string;
  verificationUrl: string;
}

export interface AppCreationProgress {
  complete: boolean;
  app?: AppRecord | null;
}

export interface AccountRecord {
  id: string;
  appRef: string;
  userOpenId: string;
  displayName: string;
  alias?: string | null;
  tenantLabel?: string | null;
  configDir: string;
  credentialOrigin: CredentialOrigin;
  health: AccountHealth;
  effectiveScopes: string[];
  lastVerifiedAt?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface AccountView {
  account: AccountRecord;
  app: AppRecord;
  active: boolean;
  runningCommands: number;
}

export interface ExistingCliCandidate {
  configDir: string;
  appId: string;
  brand: Brand;
  displayName: string;
  userOpenId: string;
  health: AccountHealth;
  alreadyImported: boolean;
}

export interface ExistingAccountImport {
  app: AppRecord;
  account: AccountRecord;
  alreadyImported: boolean;
}

export interface ActiveState {
  schemaVersion: number;
  activeAccountId?: string | null;
  managedCliPath?: string | null;
  managedCliVersion?: string | null;
  generation: number;
  updatedAt: string;
}

export interface Settings {
  pathTakeoverEnabled: boolean;
  recommendedCliVersion: string;
  scopeBatchMaxCount: number;
  scopeBatchMaxEncodedBytes: number;
}

export interface Snapshot {
  state: ActiveState;
  settings: Settings;
  accounts: AccountView[];
  apps: AppRecord[];
}

export interface ScopeBatch {
  index: number;
  modules: string[];
  scopes: string[];
  encodedBytes: number;
}

export interface AuthFlowStart {
  flowId: string;
  verificationUrl: string;
  expiresIn: number;
  qrCodePng?: number[] | null;
  batch: ScopeBatch;
  remainingScopeCount: number;
  expectedUserOpenId?: string | null;
}

export interface AuthProgress {
  complete: boolean;
  account?: AccountRecord | null;
  next?: AuthFlowStart | null;
  effectiveScopes: string[];
  missingScopes: string[];
}

export interface DiagnosticCheck {
  id: string;
  status: 'pass' | 'warn' | 'fail';
  summary: string;
  detail: string;
}

export interface DiagnosticReport {
  generatedAt: string;
  checks: DiagnosticCheck[];
}

export interface RuntimeIdentity {
  version: string;
  exePath: string;
}
