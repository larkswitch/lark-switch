use crate::error::{LpcError, Result};
use crate::model::Catalog;
use serde::Serialize;
use std::collections::BTreeSet;

pub const MAX_SINGLE_AUTH_SCOPES: usize = 250;

const CORE_SCOPES: &[&str] = &[
    "approval:approval",
    "approval:approval:readonly",
    "approval:instance:read",
    "approval:instance:write",
    "approval:task:read",
    "approval:task:write",
    "attendance:task:readonly",
    "base:app:copy",
    "base:app:create",
    "base:app:read",
    "base:app:update",
    "base:block:create",
    "base:block:delete",
    "base:block:read",
    "base:block:update",
    "base:collaborator:create",
    "base:collaborator:delete",
    "base:collaborator:read",
    "base:dashboard:copy",
    "base:dashboard:create",
    "base:dashboard:delete",
    "base:dashboard:read",
    "base:dashboard:update",
    "base:field:create",
    "base:field:delete",
    "base:field:read",
    "base:field:update",
    "base:form:create",
    "base:form:delete",
    "base:form:read",
    "base:form:update",
    "base:history:read",
    "base:record:create",
    "base:record:delete",
    "base:record:read",
    "base:record:retrieve",
    "base:record:update",
    "base:role:create",
    "base:role:delete",
    "base:role:read",
    "base:role:update",
    "base:table:create",
    "base:table:delete",
    "base:table:read",
    "base:table:update",
    "base:view:read",
    "base:view:write_only",
    "base:workflow:create",
    "base:workflow:delete",
    "base:workflow:read",
    "base:workflow:update",
    "base:workflow:write",
    "base:workspace:list",
    "board:whiteboard:node:create",
    "board:whiteboard:node:delete",
    "board:whiteboard:node:read",
    "board:whiteboard:node:update",
    "calendar:calendar:create",
    "calendar:calendar:delete",
    "calendar:calendar:read",
    "calendar:calendar:update",
    "calendar:calendar.event:create",
    "calendar:calendar.event:delete",
    "calendar:calendar.event:read",
    "calendar:calendar.event:reply",
    "calendar:calendar.event:update",
    "calendar:calendar.free_busy:read",
    "cardkit:card:read",
    "cardkit:card:write",
    "cardkit:template:read",
    "contact:user:search",
    "contact:user.basic_profile:readonly",
    "docs:doc",
    "docs:document:copy",
    "docs:document:export",
    "docs:document:import",
    "docs:document.comment:create",
    "docs:document.comment:delete",
    "docs:document.comment:read",
    "docs:document.comment:update",
    "docs:document.comment:write_only",
    "docs:document.content:read",
    "docs:document.media:download",
    "docs:document.media:upload",
    "docs:document.subscription",
    "docs:document.subscription:read",
    "docs:event:subscribe",
    "docs:event.document_deleted:read",
    "docs:event.document_edited:read",
    "docs:event.document_opened:read",
    "docs:permission.member",
    "docs:permission.member:apply",
    "docs:permission.member:auth",
    "docs:permission.member:create",
    "docs:permission.member:delete",
    "docs:permission.member:readonly",
    "docs:permission.member:retrieve",
    "docs:permission.member:transfer",
    "docs:permission.member:update",
    "docs:permission.setting",
    "docs:permission.setting:read",
    "docs:permission.setting:readonly",
    "docs:permission.setting:write_only",
    "docs:secure_label:write_only",
    "docx:document",
    "docx:document:create",
    "docx:document:readonly",
    "docx:document:write_only",
    "docx:document.block:convert",
    "drive:drive",
    "drive:drive.metadata:readonly",
    "drive:file",
    "drive:file:download",
    "drive:file:upload",
    "drive:file:view_record:readonly",
    "drive:quota_detail:read_one",
    "im:chat:create_by_user",
    "im:chat:read",
    "im:chat:update",
    "im:chat.members:read",
    "im:chat.members:write_only",
    "im:feed.flag:read",
    "im:feed.flag:write",
    "im:message",
    "im:message:readonly",
    "im:message:recall",
    "im:message.group_msg:get_as_user",
    "im:message.p2p_msg:get_as_user",
    "im:message.pins:read",
    "im:message.pins:write_only",
    "im:message.reactions:read",
    "im:message.reactions:write_only",
    "im:message.send_as_user",
    "mail:event",
    "mail:user_mailbox:readonly",
    "mail:user_mailbox.event.mail_address:read",
    "mail:user_mailbox.folder:read",
    "mail:user_mailbox.folder:write",
    "mail:user_mailbox.mail_contact:read",
    "mail:user_mailbox.mail_contact:write",
    "mail:user_mailbox.message:modify",
    "mail:user_mailbox.message:readonly",
    "mail:user_mailbox.message:send",
    "mail:user_mailbox.message.address:read",
    "mail:user_mailbox.message.body:read",
    "mail:user_mailbox.message.subject:read",
    "mail:user_mailbox.rule:read",
    "mail:user_mailbox.rule:write",
    "minutes:minutes",
    "minutes:minutes:readonly",
    "minutes:minutes:update",
    "minutes:minutes.artifacts:read",
    "minutes:minutes.basic:read",
    "minutes:minutes.media:export",
    "minutes:minutes.search:read",
    "minutes:minutes.statistics:read",
    "minutes:minutes.transcript:export",
    "minutes:minutes.upload:write",
    "offline_access",
    "okr:okr.content:readonly",
    "okr:okr.content:writeonly",
    "okr:okr.period:readonly",
    "okr:okr.progress:delete",
    "okr:okr.progress:readonly",
    "okr:okr.progress:writeonly",
    "okr:okr.progress.file:upload",
    "okr:okr.setting:read",
    "search:docs:read",
    "search:message",
    "sheets:spreadsheet",
    "sheets:spreadsheet:create",
    "sheets:spreadsheet:read",
    "sheets:spreadsheet:readonly",
    "sheets:spreadsheet:write_only",
    "sheets:spreadsheet.meta:read",
    "sheets:spreadsheet.meta:write_only",
    "slides:presentation:create",
    "slides:presentation:read",
    "slides:presentation:update",
    "slides:presentation:write_only",
    "space:document:delete",
    "space:document:move",
    "space:document:retrieve",
    "space:document:shortcut",
    "space:document.event:read",
    "space:folder:create",
    "spark:app:read",
    "spark:app:write",
    "task:attachment:write",
    "task:comment:write",
    "task:custom_field:read",
    "task:custom_field:write",
    "task:section:read",
    "task:section:write",
    "task:task:read",
    "task:task:write",
    "task:tasklist:read",
    "task:tasklist:write",
    "vc:meeting.bot.join:write",
    "vc:meeting.meetingevent:read",
    "vc:meeting.search:read",
    "vc:note:read",
    "vc:record:readonly",
    "vc:recording:read",
    "wiki:member:create",
    "wiki:member:retrieve",
    "wiki:member:update",
    "wiki:node:copy",
    "wiki:node:create",
    "wiki:node:move",
    "wiki:node:read",
    "wiki:node:retrieve",
    "wiki:node:update",
    "wiki:setting:read",
    "wiki:setting:write_only",
    "wiki:space:read",
    "wiki:space:retrieve",
    "wiki:space:write_only",
    "wiki:wiki",
    "wiki:wiki:readonly",
];

const DEPRECATED_SCOPES: &[&str] = &[
    "docs_tool:docs_tool",
    "bitable:app",
    "bitable:app:readonly",
    "event:ip_list",
    "docs:doc:readonly",
];

const OVERBROAD_SCOPES: &[&str] = &[
    "calendar:calendar",
    "calendar:calendar:readonly",
    "calendar:calendar:subscribe",
    "calendar:calendar.acl:create",
    "calendar:calendar.acl:delete",
    "calendar:calendar.acl:read",
    "calendar:exchange.bindings:create",
    "calendar:exchange.bindings:delete",
    "calendar:exchange.bindings:read",
    "calendar:settings.caldav:create",
    "calendar:settings.workhour:read",
    "calendar:time_off:create",
    "calendar:time_off:delete",
    "vc:export",
    "vc:meeting",
    "vc:meeting:readonly",
    "vc:meeting.bot.realtime:write",
    "vc:meeting.meetingid:read",
    "vc:record",
    "vc:reserve",
    "vc:reserve:readonly",
    "vc:room",
    "vc:room:readonly",
    "drive:drive:readonly",
    "drive:drive:version",
    "drive:drive:version:readonly",
    "drive:drive.search:readonly",
    "drive:export:readonly",
    "drive:file:readonly",
    "drive:file.like:readonly",
    "drive:file.meta.sec_label.read_only",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeInfo {
    pub scope: String,
    pub core: bool,
    pub reason: Option<String>,
}

fn is_core_scope(scope: &str) -> bool {
    CORE_SCOPES.contains(&scope)
}

pub fn core_scope_allowlist() -> BTreeSet<String> {
    CORE_SCOPES
        .iter()
        .map(|scope| (*scope).to_owned())
        .collect()
}

pub fn default_policy(available: &BTreeSet<String>) -> BTreeSet<String> {
    available
        .intersection(&core_scope_allowlist())
        .cloned()
        .collect()
}

/// Keep only scopes still inside the live App boundary: `policy ∩ available`.
/// Never expands a deliberate user subset back to the full default, and never
/// strips advanced scopes the user has already opted into.
pub fn clamp_policy(policy: &BTreeSet<String>, available: &BTreeSet<String>) -> BTreeSet<String> {
    policy.intersection(available).cloned().collect()
}

pub fn validate_policy_selection(selected: &BTreeSet<String>) -> Result<()> {
    if selected.len() > MAX_SINGLE_AUTH_SCOPES {
        return Err(LpcError::ScopeLimitExceeded {
            requested: selected.len(),
            limit: MAX_SINGLE_AUTH_SCOPES,
        });
    }
    Ok(())
}

/// Why a scope is excluded from the default core set. Core scopes return `None`.
pub fn exclusion_reason(scope: &str) -> Option<&'static str> {
    if is_core_scope(scope) {
        return None;
    }
    if scope.starts_with("directory:") {
        return Some("官方 CLI 没有对应命令");
    }
    if scope.starts_with("contact:") {
        return Some("涉及通讯录敏感字段");
    }
    if DEPRECATED_SCOPES.contains(&scope) {
        return Some("已被新版权限替代");
    }
    if OVERBROAD_SCOPES.contains(&scope) {
        return Some("范围过宽，已有更细粒度的替代权限");
    }
    Some("新增权限，尚未纳入默认核心")
}

pub fn scope_catalog(available: &BTreeSet<String>) -> Vec<ScopeInfo> {
    available
        .iter()
        .map(|scope| {
            let core = is_core_scope(scope);
            ScopeInfo {
                scope: scope.clone(),
                core,
                reason: if core {
                    None
                } else {
                    exclusion_reason(scope).map(str::to_owned)
                },
            }
        })
        .collect()
}

pub fn normalize_catalog(catalog: &mut Catalog) -> bool {
    let mut changed = false;
    for app in &mut catalog.apps {
        let mut normalized = clamp_policy(&app.policy_scopes, &app.available_scopes);
        // Empty policy is first-time / fully invalid only — seed defaults then.
        // A non-empty user subset must not be expanded back to full default.
        if normalized.is_empty() && !default_policy(&app.available_scopes).is_empty() {
            normalized = default_policy(&app.available_scopes);
        }
        if app.policy_scopes != normalized {
            app.policy_scopes = normalized;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppRecord, Brand, Catalog};
    use chrono::Utc;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn reviewed_core_allowlist_is_stable_and_keeps_cardkit() {
        let scopes = core_scope_allowlist();

        assert_eq!(scopes.len(), 220);
        assert!(scopes.contains("offline_access"));
        assert!(scopes.contains("cardkit:card:read"));
        assert!(scopes.contains("cardkit:card:write"));
        assert!(scopes.contains("cardkit:template:read"));
        assert!(scopes.contains("contact:user:search"));
        assert!(scopes.contains("contact:user.basic_profile:readonly"));
        assert!(!scopes.iter().any(|scope| scope.starts_with("directory:")));
        assert!(!scopes.contains("contact:user.phone:readonly"));
        assert!(!scopes.contains("calendar:exchange.bindings:read"));
        assert!(!scopes.contains("vc:room:readonly"));
        assert!(!scopes.contains("drive:file:readonly"));
        assert!(scopes.len() <= MAX_SINGLE_AUTH_SCOPES);
    }

    #[test]
    fn default_policy_intersects_live_app_boundary() {
        let available = set(&[
            "cardkit:card:read",
            "contact:user:search",
            "directory:employee:read",
            "future:unknown",
        ]);

        assert_eq!(
            default_policy(&available),
            set(&["cardkit:card:read", "contact:user:search"])
        );
    }

    #[test]
    fn clamp_policy_strips_unavailable_scopes() {
        let available = set(&[
            "cardkit:card:write",
            "contact:user:search",
            "directory:employee:read",
        ]);
        let policy = set(&[
            "cardkit:card:write",
            "directory:employee:read",
            "future:unknown",
        ]);

        assert_eq!(
            clamp_policy(&policy, &available),
            set(&["cardkit:card:write", "directory:employee:read"])
        );
    }

    #[test]
    fn clamp_policy_keeps_available_advanced_scopes() {
        let available = set(&[
            "cardkit:card:write",
            "directory:employee:read",
            "contact:user.phone:readonly",
        ]);
        let policy = set(&[
            "cardkit:card:write",
            "directory:employee:read",
            "contact:user.phone:readonly",
        ]);

        assert_eq!(clamp_policy(&policy, &available), policy);
    }

    #[test]
    fn clamp_policy_shrinks_when_scope_leaves_available() {
        let old_policy = set(&["cardkit:card:write", "contact:user:search"]);
        let new_available = set(&["cardkit:card:write", "directory:employee:read"]);

        assert_eq!(
            clamp_policy(&old_policy, &new_available),
            set(&["cardkit:card:write"])
        );
    }

    fn fixture_app(available: BTreeSet<String>, policy: BTreeSet<String>) -> Catalog {
        let now = Utc::now();
        let mut catalog = Catalog::default();
        catalog.apps.push(AppRecord {
            id: Uuid::new_v4(),
            app_id: "cli_fixture".into(),
            label: "fixture".into(),
            brand: Brand::Feishu,
            base_config_path: PathBuf::from("fixture/config.json"),
            available_scopes: available,
            policy_scopes: policy,
            scopes_observed_at: Some(now),
            created_at: now,
            updated_at: now,
        });
        catalog
    }

    #[test]
    fn normalizing_catalog_seeds_default_when_policy_is_empty() {
        let available = set(&[
            "cardkit:card:write",
            "contact:user:search",
            "directory:employee:read",
        ]);
        let mut catalog = fixture_app(available, BTreeSet::new());

        assert!(normalize_catalog(&mut catalog));
        assert_eq!(
            catalog.apps[0].policy_scopes,
            set(&["cardkit:card:write", "contact:user:search"])
        );
        assert!(!normalize_catalog(&mut catalog));
    }

    #[test]
    fn normalizing_catalog_preserves_user_subset_of_core_scopes() {
        let available = set(&[
            "cardkit:card:write",
            "contact:user:search",
            "docs:doc",
            "directory:employee:read",
        ]);
        let user_subset = set(&["cardkit:card:write", "docs:doc"]);
        let mut catalog = fixture_app(available, user_subset.clone());

        assert!(!normalize_catalog(&mut catalog));
        assert_eq!(catalog.apps[0].policy_scopes, user_subset);
    }

    #[test]
    fn normalizing_catalog_does_not_shrink_policy_with_advanced_scopes() {
        let available = set(&[
            "cardkit:card:write",
            "contact:user:search",
            "directory:employee:read",
        ]);
        let user_policy = set(&["cardkit:card:write", "directory:employee:read"]);
        let mut catalog = fixture_app(available, user_policy.clone());

        assert!(!normalize_catalog(&mut catalog));
        assert_eq!(catalog.apps[0].policy_scopes, user_policy);
    }

    #[test]
    fn manual_policy_may_include_non_core_scopes() {
        let selected = set(&["cardkit:card:read", "directory:employee:read"]);
        validate_policy_selection(&selected)
            .expect("advanced scopes inside the 250 cap are allowed");
    }

    #[test]
    fn validate_policy_allows_250_including_advanced_scopes() {
        let mut selected = BTreeSet::new();
        for index in 0..249 {
            selected.insert(format!("scope:{index}"));
        }
        selected.insert("directory:employee:read".into());
        assert_eq!(selected.len(), MAX_SINGLE_AUTH_SCOPES);
        validate_policy_selection(&selected).expect("250 scopes including advanced ones must pass");
    }

    #[test]
    fn validate_policy_rejects_251_scopes() {
        let selected: BTreeSet<String> = (0..251).map(|index| format!("scope:{index}")).collect();
        let error = validate_policy_selection(&selected).unwrap_err();
        assert_eq!(error.stable_code(), "LPC_SCOPE_LIMIT_EXCEEDED");
    }

    #[test]
    fn exclusion_reason_is_none_for_core_and_explains_directory() {
        assert_eq!(exclusion_reason("cardkit:card:read"), None);
        assert_eq!(exclusion_reason("contact:user:search"), None);
        assert_eq!(
            exclusion_reason("directory:employee:read"),
            Some("官方 CLI 没有对应命令")
        );
        assert_eq!(
            exclusion_reason("contact:user.phone:readonly"),
            Some("涉及通讯录敏感字段")
        );
        assert_eq!(
            exclusion_reason("docs:doc:readonly"),
            Some("已被新版权限替代")
        );
        assert_eq!(
            exclusion_reason("calendar:calendar"),
            Some("范围过宽，已有更细粒度的替代权限")
        );
        assert_eq!(
            exclusion_reason("future:unknown"),
            Some("新增权限，尚未纳入默认核心")
        );
    }

    #[test]
    fn scope_catalog_marks_core_and_advanced() {
        let available = set(&[
            "cardkit:card:read",
            "directory:employee:read",
            "future:unknown",
        ]);
        let catalog = scope_catalog(&available);
        assert_eq!(
            catalog
                .iter()
                .map(|item| item.scope.as_str())
                .collect::<Vec<_>>(),
            vec![
                "cardkit:card:read",
                "directory:employee:read",
                "future:unknown"
            ]
        );
        assert!(catalog[0].core);
        assert_eq!(catalog[0].reason, None);
        assert!(!catalog[1].core);
        assert_eq!(catalog[1].reason.as_deref(), Some("官方 CLI 没有对应命令"));
        assert!(!catalog[2].core);
        assert_eq!(
            catalog[2].reason.as_deref(),
            Some("新增权限，尚未纳入默认核心")
        );
    }
}
