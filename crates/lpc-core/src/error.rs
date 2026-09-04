use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum LpcError {
    #[error("LPC_NOT_INITIALIZED: Lark Profile Console has not been initialized")]
    NotInitialized,

    #[error("LPC_NO_ACTIVE_ACCOUNT: no active account is configured")]
    NoActiveAccount,

    #[error("LPC_ACCOUNT_NOT_FOUND: account {0} was not found")]
    AccountNotFound(String),

    #[error("LPC_ACCOUNT_SELECTOR_INVALID: {0}")]
    AccountSelectorInvalid(String),

    #[error("LPC_ACCOUNT_AMBIGUOUS: selector {selector} matched multiple accounts: {candidates}")]
    AccountAmbiguous {
        selector: String,
        candidates: String,
    },

    #[error(
        "LPC_ACCOUNT_ALREADY_EXISTS: account {account_id} already exists for this App and user"
    )]
    AccountAlreadyExists { account_id: String },

    #[error("LPC_APP_NOT_FOUND: app {0} was not found")]
    AppNotFound(String),

    #[error("LPC_APP_ALREADY_EXISTS: app ID {0} is already managed")]
    AppAlreadyExists(String),

    #[error("LPC_ACCOUNT_BUSY: account {account_id} has {running} running command(s)")]
    AccountBusy { account_id: String, running: usize },

    #[error("LPC_RUNTIME_MISSING: managed lark-cli is missing at {0}")]
    RuntimeMissing(PathBuf),

    #[error("LPC_RUNTIME_INCOMPATIBLE: {0}")]
    RuntimeIncompatible(String),

    #[error("LPC_RUNTIME_RECURSION: shim resolved itself as managed lark-cli")]
    RuntimeRecursion,

    #[error("LPC_CLI_FAILED: command failed with exit code {code}: {message}")]
    CliFailed { code: i32, message: String },

    #[error("LPC_CLI_TIMEOUT: official lark-cli did not finish within {0} seconds")]
    CliTimeout(u64),

    #[error(
        "LPC_CLI_KEYCHAIN_BUSY: another managed lark-cli command holds the shared keychain lock"
    )]
    CliKeychainBusy,

    #[error(
        "LPC_ROUTING_GATE_BUSY: the routing gate is still held after {0:?}; a Lark Profile Console process is stuck. Check `lpcctl ps` and the log under <LPC_HOME>\\logs."
    )]
    RoutingGateBusy(std::time::Duration),

    #[error(
        "LPC_MSIX_CONTAINER: 当前运行在 MSIX 应用容器内，会读写影子注册表并损坏飞书凭证。请在宿主终端（PowerShell / Windows Terminal）中运行 lark-cli；若确需在此环境执行，可设置 LPC_ALLOW_MSIX=1（不推荐）。"
    )]
    MsixContainerBlocked,

    #[error(
        "LPC_KEYCHAIN_VIEW_UNINITIALIZED: host keychain view marker is missing; start the installed larkswitch desktop app once before using lark-cli"
    )]
    KeychainViewUninitialized,

    #[error(
        "LPC_KEYCHAIN_VIEW_MISMATCH: this process sees a virtualized/shadow Windows registry; run lark-cli from a host terminal instead"
    )]
    KeychainViewMismatch,

    #[error("LPC_HOST_BRIDGE_UNAVAILABLE: {0}")]
    HostBridgeUnavailable(String),

    #[error("LPC_CLI_OUTPUT_INVALID: {0}")]
    InvalidCliOutput(String),

    #[error("LPC_CONFIG_UNSAFE: {0}")]
    UnsafeConfig(String),

    #[error("LPC_SCOPE_OUT_OF_BOUNDARY: requested scopes are not enabled on the app: {0:?}")]
    ScopeOutOfBoundary(Vec<String>),

    #[error(
        "LPC_SCOPE_NO_PROGRESS: authorization completed but effective scopes did not increase"
    )]
    ScopeNoProgress,

    #[error(
        "LPC_SCOPE_LIMIT_EXCEEDED: a single authorization requested {requested} scopes; limit is {limit}"
    )]
    ScopeLimitExceeded { requested: usize, limit: usize },

    #[error("LPC_SCOPE_INCOMPLETE: authorization completed without required scopes: {0:?}")]
    ScopeIncomplete(Vec<String>),

    #[error("LPC_SCOPE_POLICY_BLOCKED: scopes are disabled by the product core policy: {0:?}")]
    ScopePolicyBlocked(Vec<String>),

    #[error("LPC_SCOPE_REGRESSION: authorization removed previously effective scopes: {0:?}")]
    ScopeRegression(Vec<String>),

    #[error("LPC_AUTH_FLOW_NOT_FOUND: authorization flow {0} is not active")]
    AuthFlowNotFound(String),

    #[error("LPC_AUTH_FLOW_EXPIRED: authorization flow has expired")]
    AuthFlowExpired,

    #[error("LPC_AUTH_IDENTITY_MISMATCH: expected {expected}, received {actual}")]
    AuthIdentityMismatch { expected: String, actual: String },

    #[error("LPC_PATH_TAKEOVER_FAILED: {0}")]
    PathTakeover(String),

    #[error("LPC_INTEGRITY_FAILED: {0}")]
    Integrity(String),

    #[error("LPC_IO: {0}")]
    Io(#[from] std::io::Error),

    #[error("LPC_JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("LPC_HTTP: {0}")]
    Http(#[from] reqwest::Error),

    #[error("LPC_SEMVER: {0}")]
    Semver(#[from] semver::Error),

    #[error("LPC_INTERNAL: {0}")]
    Internal(String),
}

impl LpcError {
    pub fn stable_code(&self) -> &'static str {
        match self {
            Self::NotInitialized => "LPC_NOT_INITIALIZED",
            Self::NoActiveAccount => "LPC_NO_ACTIVE_ACCOUNT",
            Self::AccountNotFound(_) => "LPC_ACCOUNT_NOT_FOUND",
            Self::AccountSelectorInvalid(_) => "LPC_ACCOUNT_SELECTOR_INVALID",
            Self::AccountAmbiguous { .. } => "LPC_ACCOUNT_AMBIGUOUS",
            Self::AccountAlreadyExists { .. } => "LPC_ACCOUNT_ALREADY_EXISTS",
            Self::AppNotFound(_) => "LPC_APP_NOT_FOUND",
            Self::AppAlreadyExists(_) => "LPC_APP_ALREADY_EXISTS",
            Self::AccountBusy { .. } => "LPC_ACCOUNT_BUSY",
            Self::RuntimeMissing(_) => "LPC_RUNTIME_MISSING",
            Self::RuntimeIncompatible(_) => "LPC_RUNTIME_INCOMPATIBLE",
            Self::RuntimeRecursion => "LPC_RUNTIME_RECURSION",
            Self::CliFailed { .. } => "LPC_CLI_FAILED",
            Self::CliTimeout(_) => "LPC_CLI_TIMEOUT",
            Self::CliKeychainBusy => "LPC_CLI_KEYCHAIN_BUSY",
            Self::RoutingGateBusy(_) => "LPC_ROUTING_GATE_BUSY",
            Self::MsixContainerBlocked => "LPC_MSIX_CONTAINER",
            Self::KeychainViewUninitialized => "LPC_KEYCHAIN_VIEW_UNINITIALIZED",
            Self::KeychainViewMismatch => "LPC_KEYCHAIN_VIEW_MISMATCH",
            Self::HostBridgeUnavailable(_) => "LPC_HOST_BRIDGE_UNAVAILABLE",
            Self::InvalidCliOutput(_) => "LPC_CLI_OUTPUT_INVALID",
            Self::UnsafeConfig(_) => "LPC_CONFIG_UNSAFE",
            Self::ScopeOutOfBoundary(_) => "LPC_SCOPE_OUT_OF_BOUNDARY",
            Self::ScopeNoProgress => "LPC_SCOPE_NO_PROGRESS",
            Self::ScopeLimitExceeded { .. } => "LPC_SCOPE_LIMIT_EXCEEDED",
            Self::ScopeIncomplete(_) => "LPC_SCOPE_INCOMPLETE",
            Self::ScopePolicyBlocked(_) => "LPC_SCOPE_POLICY_BLOCKED",
            Self::ScopeRegression(_) => "LPC_SCOPE_REGRESSION",
            Self::AuthFlowNotFound(_) => "LPC_AUTH_FLOW_NOT_FOUND",
            Self::AuthFlowExpired => "LPC_AUTH_FLOW_EXPIRED",
            Self::AuthIdentityMismatch { .. } => "LPC_AUTH_IDENTITY_MISMATCH",
            Self::PathTakeover(_) => "LPC_PATH_TAKEOVER_FAILED",
            Self::Integrity(_) => "LPC_INTEGRITY_FAILED",
            Self::Io(_) => "LPC_IO",
            Self::Json(_) => "LPC_JSON",
            Self::Http(_) => "LPC_HTTP",
            Self::Semver(_) => "LPC_SEMVER",
            Self::Internal(_) => "LPC_INTERNAL",
        }
    }

    /// Process exit code for LPC control-plane / shim failures.
    /// Selector and missing-account user errors are 64; internal failures are 70.
    /// Official CLI exit codes are returned separately by the shim.
    pub fn process_exit_code(&self) -> i32 {
        match self {
            Self::NotInitialized
            | Self::NoActiveAccount
            | Self::AccountNotFound(_)
            | Self::AccountSelectorInvalid(_)
            | Self::AccountAmbiguous { .. }
            | Self::AppNotFound(_)
            | Self::AccountBusy { .. } => 64,
            _ => 70,
        }
    }
}

pub type Result<T> = std::result::Result<T, LpcError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_new_account_has_stable_code() {
        let error = LpcError::AccountAlreadyExists {
            account_id: "account-id".into(),
        };

        assert_eq!(error.stable_code(), "LPC_ACCOUNT_ALREADY_EXISTS");
    }

    #[test]
    fn auth_flow_expired_has_stable_code() {
        assert_eq!(
            LpcError::AuthFlowExpired.stable_code(),
            "LPC_AUTH_FLOW_EXPIRED"
        );
    }

    #[test]
    fn scope_limit_has_stable_code() {
        let error = LpcError::ScopeLimitExceeded {
            requested: 251,
            limit: 250,
        };

        assert_eq!(error.stable_code(), "LPC_SCOPE_LIMIT_EXCEEDED");
    }

    #[test]
    fn blocked_scope_policy_has_stable_code() {
        let error = LpcError::ScopePolicyBlocked(vec!["directory:employee:read".into()]);

        assert_eq!(error.stable_code(), "LPC_SCOPE_POLICY_BLOCKED");
    }
}
