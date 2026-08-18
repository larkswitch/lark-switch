use crate::error::{LpcError, Result};

/// Returns true when the current process runs inside an MSIX/AppX package identity.
pub fn is_running_in_msix_package() -> bool {
    if std::env::var_os("LPC_TEST_MSIX").as_deref() == Some(std::ffi::OsStr::new("1")) {
        return true;
    }
    detect_msix_platform()
}

#[cfg(windows)]
fn detect_msix_platform() -> bool {
    detect_msix_via_api()
}

#[cfg(not(windows))]
fn detect_msix_platform() -> bool {
    false
}

/// Shim entry guard: block MSIX containers unless explicitly overridden.
pub fn enforce_msix_shim_policy() -> Result<()> {
    if !is_running_in_msix_package() {
        return Ok(());
    }
    if std::env::var_os("LPC_ALLOW_MSIX").as_deref() == Some(std::ffi::OsStr::new("1")) {
        eprintln!(
            "[LPC_MSIX_WARNING] 当前运行在 MSIX 应用容器内，凭证会写入影子注册表并可能与宿主环境分叉；已因 LPC_ALLOW_MSIX=1 放行。"
        );
        return Ok(());
    }
    Err(LpcError::MsixContainerBlocked)
}

#[cfg(windows)]
fn detect_msix_via_api() -> bool {
    use std::ptr;

    const APPMODEL_ERROR_NO_PACKAGE: i32 = 15700;
    const ERROR_INSUFFICIENT_BUFFER: i32 = 122;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentPackageFullName(
            package_full_name_length: *mut u32,
            package_full_name: *mut u16,
        ) -> i32;
    }

    unsafe {
        let mut length: u32 = 0;
        let status = GetCurrentPackageFullName(&mut length, ptr::null_mut());
        match status {
            APPMODEL_ERROR_NO_PACKAGE => false,
            ERROR_INSUFFICIENT_BUFFER => length > 0,
            0 => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msix_detection_respects_test_override() {
        std::env::set_var("LPC_TEST_MSIX", "1");
        assert!(is_running_in_msix_package());
        std::env::remove_var("LPC_TEST_MSIX");
        assert!(!is_running_in_msix_package());
    }

    #[test]
    fn msix_policy_allows_escape_hatch() {
        std::env::set_var("LPC_TEST_MSIX", "1");
        std::env::set_var("LPC_ALLOW_MSIX", "1");
        assert!(enforce_msix_shim_policy().is_ok());
        std::env::remove_var("LPC_ALLOW_MSIX");
        std::env::remove_var("LPC_TEST_MSIX");
    }

    #[test]
    fn msix_policy_blocks_without_override() {
        std::env::set_var("LPC_TEST_MSIX", "1");
        std::env::remove_var("LPC_ALLOW_MSIX");
        let error = enforce_msix_shim_policy().unwrap_err();
        assert_eq!(error.stable_code(), "LPC_MSIX_CONTAINER");
        std::env::remove_var("LPC_TEST_MSIX");
    }
}
