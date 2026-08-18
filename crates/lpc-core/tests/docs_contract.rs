//! Documentation contract: the docs and the implementation must move together.
//!
//! Every drift this test was written for was real. `KEYCHAIN-DURABILITY.md`
//! advertised an `lpcctl diagnose` that clap never defined, it recommended the
//! whole-hive restore that caused the 2026-07-22 cascade *after* that practice
//! became a red line, and README documented about half of the CLI. The compiler
//! sees none of it, and a reader has no way to tell a stale instruction from a
//! current one — a wrong instruction in an incident-response document is worse
//! than no document.
//!
//! Like the other contracts in this directory, nothing is executed: sources and
//! documents are read as text.

mod common;

use std::collections::BTreeSet;
use std::path::Path;

/// Documents that describe how the product works *today* — the public set.
///
/// `AGENTS.md` is a local operator memo that must not be load-bearing for the
/// public repository, so nothing here may depend on it being scanned.
/// `docs/TOKEN-EXPIRY-INVESTIGATION-2026-07-22.md` and
/// `docs/STATIC-CONTRACTS-2026-07-27.md` are historical incident reports: they
/// record what was true on a date, quote commands that have since been
/// retired, and pin line numbers that any refactor invalidates; auditing them
/// would turn this test into a maintenance tax.
/// `UNTRACKED-FILE-TRIAGE-2026-07-27.md` (an inventory) and
/// `REFERENCE-lark-coding-agent-bridge-2026-07-27.md` (analysis of a
/// different codebase) are out for a related reason: both quote strings they do
/// not own, so any assertion over them would fire on someone else's vocabulary.
const SCANNED_DOCS: &[&str] = &[
    "README.md",
    "README.en.md",
    "docs/ARCHITECTURE.md",
    "docs/KEYCHAIN-DURABILITY.md",
    "docs/MANUAL-E2E.md",
    "docs/PRODUCT.md",
    "docs/RELEASE.md",
    "docs/SECURITY.md",
    "docs/TESTING.md",
];

/// The single source of truth for what `lpcctl` accepts.
const LPCCTL_MAIN: &str = "crates/lpcctl/src/main.rs";

const KEYCHAIN_DOC: &str = "docs/KEYCHAIN-DURABILITY.md";

/// `lpcctl account alias set` is the deepest command path in the tree.
const MAX_SUBCOMMAND_DEPTH: usize = 3;

/// Subcommands that documentation must stop advertising.
///
/// `diagnose` never existed in clap; only `doctor` does. Readers copy commands
/// out of docs verbatim, so a name that only ever produced a clap usage error
/// has to disappear rather than linger as a synonym.
const RETIRED_SUBCOMMANDS: &[&str] = &["diagnose"];

/// Strings the docs and the code must agree on, in both directions.
struct PairedIdentifier {
    /// Spelling used in prose.
    literal: &'static str,
    /// Documents that must all mention it.
    docs: &'static [&'static str],
    /// Source file that must contain `code_spelling`, when one applies.
    code: Option<&'static str>,
    /// Spelling in the source, when it differs from `literal`.
    code_spelling: Option<&'static str>,
}

const PAIRED_IDENTIFIERS: &[PairedIdentifier] = &[
    PairedIdentifier {
        literal: "LPC_REQUIRE_DEPLOY_ARTIFACT",
        docs: &["docs/TESTING.md"],
        code: Some("crates/lpc-core/tests/deploy_contract.rs"),
        code_spelling: None,
    },
    PairedIdentifier {
        literal: "lpc-allow-raw-write:",
        docs: &["docs/TESTING.md"],
        code: Some("crates/lpc-core/tests/atomic_write_contract.rs"),
        code_spelling: None,
    },
    PairedIdentifier {
        literal: "--account",
        docs: &["README.md", "docs/ARCHITECTURE.md"],
        code: Some("crates/lpc-core/src/selector.rs"),
        code_spelling: None,
    },
    PairedIdentifier {
        literal: "--lpc-account",
        docs: &["README.md", "docs/ARCHITECTURE.md"],
        code: Some("crates/lpc-core/src/selector.rs"),
        code_spelling: None,
    },
    PairedIdentifier {
        literal: "LARKSWITCH_ACCOUNT",
        docs: &["README.md"],
        code: Some("crates/lpc-shim/src/main.rs"),
        code_spelling: None,
    },
    PairedIdentifier {
        literal: "LPC_ACCOUNT",
        docs: &["README.md"],
        code: Some("crates/lpc-shim/src/main.rs"),
        code_spelling: None,
    },
    PairedIdentifier {
        literal: "LARKSUITE_CLI_CONFIG_DIR",
        docs: &["README.md", "docs/ARCHITECTURE.md"],
        code: Some("crates/lpc-shim/src/main.rs"),
        code_spelling: None,
    },
    PairedIdentifier {
        literal: "LPC_HOME",
        docs: &["README.md"],
        code: Some("crates/lpc-core/src/paths.rs"),
        code_spelling: None,
    },
    PairedIdentifier {
        // clap derives the flag from the field name, so the source spells it
        // `secret_stdin` and the flag `--secret-stdin` exists nowhere in it.
        literal: "--secret-stdin",
        docs: &["README.md", "docs/SECURITY.md"],
        code: Some(LPCCTL_MAIN),
        code_spelling: Some("secret_stdin"),
    },
    PairedIdentifier {
        // Retention is documented by constant name so tuning it cannot silently
        // date the write-up; the 30-vs-240 mismatch is exactly how it went wrong.
        literal: "MAX_RETAINED_KEYCHAIN_BACKUPS",
        docs: &["docs/KEYCHAIN-DURABILITY.md"],
        code: Some("crates/lpc-core/src/keychain_guard.rs"),
        code_spelling: Some("const MAX_RETAINED_KEYCHAIN_BACKUPS"),
    },
    PairedIdentifier {
        // The one build command that produces a deployable desktop binary.
        literal: "npx tauri build --no-bundle",
        docs: &["docs/TESTING.md"],
        code: None,
        code_spelling: None,
    },
];

/// The contract test files that guard the release rules, including this one.
const CONTRACT_TEST_FILES: &[&str] = &[
    "crates/lpc-core/tests/deploy_contract.rs",
    "crates/lpc-core/tests/atomic_write_contract.rs",
    "crates/lpc-core/tests/keychain_contract.rs",
    "crates/lpc-core/tests/docs_contract.rs",
];

/// Whole-hive restore, in every spelling that could be copy-pasted.
///
/// The keychain document must not carry these even as advice: a reader in the
/// middle of an outage reaches for the first runnable command they find.
const FORBIDDEN_RESTORE_ADVICE: &[&str] = &[
    "reg import",
    "reg.exe import",
    "reg restore",
    "regedit /s",
    "regedit.exe /s",
    "regedit /i",
    "import-registryfile",
    "整表恢复",
    "整表 import",
];

/// What the keychain document must say instead.
const REQUIRED_RESTORE_GUIDANCE: &[(&str, &str)] = &[
    ("restore-lark-keychain.ps1", "the sanctioned restore tool"),
    (
        "-ValueName",
        "naming each value to write back, one at a time",
    ),
    ("-Apply", "the switch that turns the dry run into a write"),
    ("干跑", "that the default run writes nothing"),
];

#[test]
fn documented_lpcctl_subcommands_exist_in_the_clap_definition() {
    let root = common::repo_root();
    let known = clap_subcommands(&read(&root, LPCCTL_MAIN));

    // A parser that silently returns nothing would make every other assertion
    // here pass for the wrong reason.
    assert!(
        known.contains("doctor") && known.contains("import-config"),
        "could not parse a plausible subcommand set out of {LPCCTL_MAIN} (got {known:?}). \
         The enum layout changed; update clap_subcommands()."
    );

    let mut violations = Vec::new();
    for doc in SCANNED_DOCS {
        for (line, token) in documented_subcommands(&read(&root, doc)) {
            if !known.contains(&token) {
                violations.push(format!("  {doc}:{line} — documents `lpcctl {token}`"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "documented lpcctl subcommands that do not exist:\n{}\n\n\
         Actual subcommands, derived from the `#[derive(Subcommand)]` enums in \
         {LPCCTL_MAIN}: {}.\n\
         Either the document names a command that was never implemented — delete it — \
         or a command was renamed and the document was left behind.",
        violations.join("\n"),
        known
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    );
}

#[test]
fn load_bearing_identifiers_appear_on_both_sides() {
    let root = common::repo_root();
    let mut violations = Vec::new();

    for pair in PAIRED_IDENTIFIERS {
        let literal = pair.literal;
        for doc in pair.docs {
            if !read(&root, doc).contains(literal) {
                violations.push(format!(
                    "  {doc} no longer mentions `{literal}`; the code still relies on it"
                ));
            }
        }
        let Some(source) = pair.code else {
            continue;
        };
        let spelling = pair.code_spelling.unwrap_or(literal);
        if !read(&root, source).contains(spelling) {
            violations.push(format!(
                "  {source} no longer contains `{spelling}`, but {} still documents `{literal}`",
                pair.docs.join(" / ")
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "documentation and code disagree about load-bearing identifiers:\n{}\n\n\
         Each of these names an environment variable, flag or marker that a reader \
         is told to type. Rename it on both sides in the same change, or update this \
         table if the identifier is genuinely gone.",
        violations.join("\n")
    );
}

#[test]
fn contract_test_files_exist() {
    let root = common::repo_root();
    let missing: Vec<&str> = CONTRACT_TEST_FILES
        .iter()
        .copied()
        .filter(|relative| !root.join(relative).is_file())
        .collect();

    assert!(
        missing.is_empty(),
        "contract test files that guard the release rules are missing: {missing:?}.\n\
         Restore the file or update CONTRACT_TEST_FILES — a guard that other \
         documents promise but that does not exist is worse than an undocumented \
         one, because people stop looking."
    );
}

/// `--q` reads like a typo for `--query`, so somebody will eventually "fix" the
/// README and hand users a flag clap rejects. The spelling comes from
/// `#[arg(short, long)] q` in the `Search` variant.
#[test]
fn readme_spells_the_account_search_flag_the_way_clap_does() {
    let root = common::repo_root();
    let readme = read(&root, "README.md");

    assert!(
        !readme.contains("--query"),
        "README.md documents `--query`, but `lpcctl account search` only accepts `--q` \
         (clap derives it from the `q` field in {LPCCTL_MAIN}). Change the README back, \
         or rename the field and this assertion together."
    );
    assert!(
        contains_flag(&readme, "--q"),
        "README.md no longer documents the `--q` flag of `lpcctl account search`. \
         If the flag was renamed, update {LPCCTL_MAIN}, the README and this assertion \
         in the same change."
    );
}

#[test]
fn retired_lpcctl_subcommands_are_gone_from_the_docs() {
    let root = common::repo_root();
    let mut violations = Vec::new();

    for doc in SCANNED_DOCS {
        let text = read(&root, doc);
        for (offset, line) in text.lines().enumerate() {
            let lowered = line.to_ascii_lowercase();
            if !lowered.contains("lpcctl") {
                continue;
            }
            for retired in RETIRED_SUBCOMMANDS {
                if mentions_word(&lowered, retired) {
                    violations.push(format!("  {doc}:{} — `{retired}`", offset + 1));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "retired lpcctl subcommands still documented:\n{}\n\n\
         These names produce a clap usage error. `lpcctl doctor` is the only \
         diagnostics entry point; drop the alias instead of listing it as one.",
        violations.join("\n")
    );
}

#[test]
fn keychain_doc_matches_the_current_restore_red_lines() {
    let root = common::repo_root();
    let text = read(&root, KEYCHAIN_DOC);
    let mut violations = Vec::new();

    for (offset, line) in text.lines().enumerate() {
        let lowered = line.to_ascii_lowercase();
        for pattern in FORBIDDEN_RESTORE_ADVICE {
            if lowered.contains(pattern) {
                violations.push(format!(
                    "  {KEYCHAIN_DOC}:{} — whole-hive restore (`{pattern}`)",
                    offset + 1
                ));
            }
        }
    }

    for (needle, why) in REQUIRED_RESTORE_GUIDANCE {
        if !text.contains(needle) {
            violations.push(format!(
                "  {KEYCHAIN_DOC} no longer documents `{needle}` — {why}"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "keychain recovery guidance drifted from the AGENTS.md red lines:\n{}\n\n\
         Replaying a snapshot over the whole key overwrites healthy accounts with \
         refresh tokens Lark has already rotated; the server answers 20064 and the \
         official CLI deletes the credential (2026-07-22). Recovery is one named \
         value at a time via scripts/restore-lark-keychain.ps1: dry run by default, \
         `-ValueName` per value, `-Apply` to write, safety snapshot first. Describe \
         the forbidden operation in words rather than pasting a runnable command.",
        violations.join("\n")
    );
}

fn read(root: &Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

/// Subcommand names clap derives from the `#[derive(Subcommand)]` enums.
///
/// Reading the enums keeps the set in step with the code; a hand-maintained list
/// would be one more thing to forget when a command is renamed. clap lowercases
/// variants into kebab-case, so `ImportConfig` is the CLI's `import-config`.
fn clap_subcommands(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut derives_subcommand = false;
    let mut inside = false;

    for line in source.lines() {
        if line.starts_with("#[derive(") {
            derives_subcommand = line.contains("Subcommand");
        } else if let Some(rest) = line.strip_prefix("enum ") {
            inside = derives_subcommand && rest.ends_with('{');
            derives_subcommand = false;
        } else if line.starts_with('}') {
            inside = false;
        } else if inside {
            if let Some(variant) = top_level_variant(line) {
                names.insert(kebab_case(&variant));
            }
        }
    }

    names
}

/// A variant declared directly in an enum body: exactly one indent level, an
/// upper-camel name, then `,`, `{` or `(`. Struct fields sit deeper and start
/// lowercase, attributes start with `#`.
fn top_level_variant(line: &str) -> Option<String> {
    let rest = line.strip_prefix("    ")?;
    if !rest.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    let name: String = rest
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    let tail = rest[name.len()..].trim_start();
    ["{", ",", "("]
        .iter()
        .any(|opener| tail.starts_with(opener))
        .then_some(name)
}

fn kebab_case(variant: &str) -> String {
    let mut out = String::with_capacity(variant.len() + 2);
    for (index, character) in variant.char_indices() {
        if character.is_ascii_uppercase() && index > 0 {
            out.push('-');
        }
        out.push(character.to_ascii_lowercase());
    }
    out
}

/// `lpcctl` invocations documented in a Markdown code span or fenced block, as
/// `(line number, subcommand token)` pairs.
///
/// Only code-formatted text counts. Prose around a command would otherwise be
/// parsed as arguments — "`lpcctl account list` contains distinct Open IDs"
/// would report a subcommand named `contains`.
fn documented_subcommands(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut fenced = false;

    for (offset, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            collect_invocations(line, offset + 1, &mut found);
        } else {
            for span in line.split('`').skip(1).step_by(2) {
                collect_invocations(span, offset + 1, &mut found);
            }
        }
    }

    found
}

fn collect_invocations(segment: &str, line: usize, found: &mut Vec<(usize, String)>) {
    let tokens: Vec<&str> = segment.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        // `target/debug/lpcctl app import` is as much an invocation as `lpcctl app import`.
        if !(*token == "lpcctl"
            || *token == "larkswitch"
            || token.ends_with("/lpcctl")
            || token.ends_with("\\lpcctl")
            || token.ends_with("/larkswitch")
            || token.ends_with("\\larkswitch"))
        {
            continue;
        }
        for candidate in tokens.iter().skip(index + 1).take(MAX_SUBCOMMAND_DEPTH) {
            if !is_subcommand_token(candidate) {
                break;
            }
            found.push((line, (*candidate).to_owned()));
        }
    }
}

/// Flags, placeholders such as `<APP_UUID>`, selectors such as `alias:name` and
/// anything non-ASCII end the command path.
fn is_subcommand_token(token: &str) -> bool {
    token.starts_with(|c: char| c.is_ascii_lowercase())
        && token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `needle` present as a whole flag, not as the prefix of a longer one.
fn contains_flag(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(
        |(index, _)| match text[index + needle.len()..].chars().next() {
            Some(next) => !next.is_ascii_alphanumeric() && next != '-',
            None => true,
        },
    )
}

/// `word` present as a standalone word, so `diagnose-lpc-env.ps1` (a real
/// script) does not read as the retired `diagnose` subcommand.
fn mentions_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(index, _)| {
        let before = haystack[..index].chars().next_back();
        let after = haystack[index + word.len()..].chars().next();
        !before.is_some_and(is_word_character) && !after.is_some_and(is_word_character)
    })
}

fn is_word_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_'
}
