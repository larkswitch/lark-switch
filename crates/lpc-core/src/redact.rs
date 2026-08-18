//! Redaction for anything the product prints, logs, or hands back to a user.
//!
//! Two levels, because the two audiences need different things. A local
//! diagnostic is read by the person whose machine it describes: absolute paths
//! and account labels are the reason they ran it, and stripping those makes the
//! report worthless. The same report pasted into a ticket or a group chat has
//! left that machine, and the Windows account name embedded in every path is
//! then gratuitous. Credentials are removed at both levels.
//!
//! Deliberately *not* redacted at either level: account display names. They are
//! labels the user chose, and "which account is broken" is the whole question a
//! shared diagnostic is meant to answer.

use once_cell::sync::Lazy;
use regex::{Captures, Regex};

/// How far to go. See the module comment for why one level is not enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionLevel {
    /// Stays on the machine it describes. Credentials out, context intact.
    Local,
    /// Leaves the machine. Also drops host-identifying detail and caps length.
    Outbound,
}

/// Longest string an outbound payload may carry. A machine-wide PATH easily
/// runs into the thousands of characters and buries the actual finding.
const MAX_OUTBOUND_LEN: usize = 4096;

/// Layer one: a credential named by its key. Both `key=value` and JSON forms
/// are covered, including JSON that has been through `to_string` twice and so
/// arrives as `\"key\":\"value\"` — a shape the previous single-escape pattern
/// silently let through.
static KEYED_SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?ix)
        ( access[_-]?token | refresh[_-]?token | app[_-]?secret | client[_-]?secret
        | device[_-]?code   | user[_-]?code     | authorization  | api[_-]?key
        | password | passwd | secret | token | credential | cookie )
        ( \\?"?\s*[=:]\s*\\?"? )
        ( [^\s",}\\]+ )
        "#,
    )
    .unwrap()
});

/// Layer two: credentials recognisable by shape alone, with no key name to go
/// on. Without this a bare token pasted into an error message survives.
static BEARER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]+").unwrap());
static JWT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}").unwrap()
});
static VERIFICATION_URI: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)(verification_uri_complete\\?"\s*:\s*\\?")[^"\\]+"#).unwrap());
static OPAQUE_BLOB: Lazy<Regex> = Lazy::new(|| Regex::new(r"[A-Za-z0-9+/_-]{48,}={0,2}").unwrap());

/// Host-identifying paths, stripped only on the way out.
/// Backslashes are doubled once the path has been through a JSON encoder, which
/// is exactly the form log records arrive in, so both spellings must match.
static WINDOWS_HOME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)([A-Za-z]:\\{1,2}Users\\{1,2})[^\\/:*?"<>|\r\n]+"#).unwrap());
static UNIX_HOME: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(/(?:home|Users)/)[^/\s:"]+"#).unwrap());

/// Redact for local consumption. Kept as the default so existing call sites
/// that reason about their own machine keep their context.
pub fn redact_text(input: &str) -> String {
    redact_with(RedactionLevel::Local, input)
}

pub fn redact_with(level: RedactionLevel, input: &str) -> String {
    let mut output = KEYED_SECRET
        .replace_all(input, "$1$2[REDACTED]")
        .into_owned();
    output = BEARER.replace_all(&output, "$1[REDACTED]").into_owned();
    output = JWT.replace_all(&output, "[REDACTED]").into_owned();
    output = VERIFICATION_URI
        .replace_all(&output, "$1[REDACTED]")
        .into_owned();
    output = OPAQUE_BLOB
        .replace_all(&output, |caps: &Captures| {
            let blob = &caps[0];
            if looks_opaque(blob) {
                "[REDACTED]".to_owned()
            } else {
                blob.to_owned()
            }
        })
        .into_owned();

    if level == RedactionLevel::Local {
        return output;
    }

    output = WINDOWS_HOME
        .replace_all(&output, "$1[REDACTED_USER]")
        .into_owned();
    output = UNIX_HOME
        .replace_all(&output, "$1[REDACTED_USER]")
        .into_owned();
    truncate(output)
}

/// A long run of base64url characters is only interesting if it mixes cases and
/// digits. Requiring the mix is what keeps hex digests, long file names and
/// run-on words out of the redactor — those stay readable, and a diagnostic
/// that redacts its own SHA-256 values helps nobody.
fn looks_opaque(blob: &str) -> bool {
    blob.chars().any(|c| c.is_ascii_digit())
        && blob.chars().any(|c| c.is_ascii_lowercase())
        && blob.chars().any(|c| c.is_ascii_uppercase())
}

fn truncate(mut text: String) -> String {
    if text.len() <= MAX_OUTBOUND_LEN {
        return text;
    }
    let mut cut = MAX_OUTBOUND_LEN;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let dropped = text.len() - cut;
    text.truncate(cut);
    text.push_str(&format!("… [{dropped} more characters omitted]"));
    text
}

/// Cheap pre-check for callers that want to refuse to emit something at all
/// rather than emit a redacted version of it.
pub fn contains_likely_secret(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    [
        "access_token",
        "refresh_token",
        "app_secret",
        "client_secret",
        "device_code",
        "user_code",
        "authorization",
        "api_key",
        "password",
        "credential",
        "verification_uri_complete",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_secret_shapes() {
        let source = r#"{"access_token":"u-abc","refresh_token":"r-def","safe":"ok"}"#;
        let redacted = redact_text(source);
        assert!(!redacted.contains("u-abc"));
        assert!(!redacted.contains("r-def"));
        assert!(redacted.contains("\"safe\":\"ok\""));
    }

    #[test]
    fn redacts_secrets_that_survived_a_second_json_encode() {
        let source = r#"{\"app_secret\":\"s-hunter2\",\"tenant\":\"acme\"}"#;
        let redacted = redact_text(source);
        assert!(!redacted.contains("s-hunter2"));
        assert!(redacted.contains("acme"));
    }

    #[test]
    fn redacts_credentials_that_have_no_key_name() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r";
        assert!(!redact_text(jwt).contains(jwt));
        assert!(!redact_text("Bearer aXNzdWVkLXRva2Vu").contains("aXNzdWVkLXRva2Vu"));

        let blob = "Zm9vYmFyQmF6UXV1eDEyMzQ1Njc4OTBhYmNkZWZnaGlqa2xtbm9wUXJzdHV2";
        assert!(!redact_text(blob).contains(blob));
    }

    #[test]
    fn keeps_digests_and_paths_readable() {
        // All-lowercase hex: a digest, not a credential.
        let digest = "ba8b12e291718405915afc478ee0f0e8895707fb14da86b2824b2bad8493a5ae";
        assert!(redact_text(digest).contains(digest));

        let path = r"C:\Users\someone\AppData\Local\Lark Profile Console";
        assert!(redact_text(path).contains("someone"));
    }

    #[test]
    fn outbound_drops_the_account_name_from_paths() {
        let path = r"C:\Users\someone\AppData\Local\LarkProfileConsole";
        let shared = redact_with(RedactionLevel::Outbound, path);
        assert!(!shared.contains("someone"));
        assert!(shared.contains("[REDACTED_USER]"));
        assert!(shared.contains("AppData"));

        let unix = redact_with(RedactionLevel::Outbound, "/home/someone/.lark-cli");
        assert!(!unix.contains("someone"));
        assert!(unix.contains(".lark-cli"));
    }

    #[test]
    fn outbound_caps_runaway_strings() {
        let long = "a".repeat(MAX_OUTBOUND_LEN * 2);
        let shared = redact_with(RedactionLevel::Outbound, &long);
        assert!(shared.len() < long.len());
        assert!(shared.contains("more characters omitted"));
        assert_eq!(redact_with(RedactionLevel::Local, &long).len(), long.len());
    }
}
