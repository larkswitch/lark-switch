use crate::error::{LpcError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScopeBatch {
    pub index: usize,
    pub modules: Vec<String>,
    pub scopes: BTreeSet<String>,
    pub encoded_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationPlan {
    pub boundary: BTreeSet<String>,
    pub target: BTreeSet<String>,
    pub effective: BTreeSet<String>,
    pub remaining: BTreeSet<String>,
    pub batches: Vec<ScopeBatch>,
}

impl ScopeBatch {
    pub fn from_scopes(index: usize, scopes: BTreeSet<String>) -> Self {
        let modules = scopes
            .iter()
            .map(|scope| module_of(scope))
            .collect::<BTreeSet<_>>();
        Self {
            index,
            modules: modules.into_iter().collect(),
            encoded_bytes: encoded_scope_bytes(&scopes),
            scopes,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScopePlanner {
    max_count: usize,
    max_encoded_bytes: usize,
}

impl ScopePlanner {
    pub fn new(max_count: usize, max_encoded_bytes: usize) -> Self {
        Self {
            max_count: max_count.max(1),
            max_encoded_bytes: max_encoded_bytes.max(64),
        }
    }

    pub fn plan(
        &self,
        boundary: &BTreeSet<String>,
        selected: &BTreeSet<String>,
        effective: &BTreeSet<String>,
    ) -> Result<AuthorizationPlan> {
        let out_of_boundary: Vec<String> = selected.difference(boundary).cloned().collect();
        if !out_of_boundary.is_empty() {
            return Err(LpcError::ScopeOutOfBoundary(out_of_boundary));
        }

        let target = selected.clone();
        let remaining: BTreeSet<String> = target.difference(effective).cloned().collect();
        let batches = self.make_batches(&remaining);
        Ok(AuthorizationPlan {
            boundary: boundary.clone(),
            target,
            effective: effective.clone(),
            remaining,
            batches,
        })
    }

    pub fn next_batch(
        &self,
        boundary: &BTreeSet<String>,
        target: &BTreeSet<String>,
        effective: &BTreeSet<String>,
    ) -> Result<Option<ScopeBatch>> {
        Ok(self
            .plan(boundary, target, effective)?
            .batches
            .into_iter()
            .next())
    }

    fn make_batches(&self, remaining: &BTreeSet<String>) -> Vec<ScopeBatch> {
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for scope in remaining {
            groups
                .entry(module_of(scope))
                .or_default()
                .push(scope.clone());
        }

        let mut raw_batches: Vec<(BTreeSet<String>, BTreeSet<String>)> = Vec::new();
        let mut current_scopes = BTreeSet::new();
        let mut current_modules = BTreeSet::new();

        for (module, scopes) in groups {
            if fits(
                &current_scopes,
                &scopes,
                self.max_count,
                self.max_encoded_bytes,
            ) {
                current_modules.insert(module);
                current_scopes.extend(scopes);
                continue;
            }
            if !current_scopes.is_empty() {
                raw_batches.push((current_modules, current_scopes));
                current_modules = BTreeSet::new();
                current_scopes = BTreeSet::new();
            }

            if fits(
                &current_scopes,
                &scopes,
                self.max_count,
                self.max_encoded_bytes,
            ) {
                current_modules.insert(module);
                current_scopes.extend(scopes);
                continue;
            }

            // A single business module is larger than the conservative budget.
            // Split it deterministically while keeping the module label visible.
            for scope in scopes {
                if !fits(
                    &current_scopes,
                    std::slice::from_ref(&scope),
                    self.max_count,
                    self.max_encoded_bytes,
                ) && !current_scopes.is_empty()
                {
                    raw_batches.push((current_modules, current_scopes));
                    current_modules = BTreeSet::new();
                    current_scopes = BTreeSet::new();
                }
                current_modules.insert(module.clone());
                current_scopes.insert(scope);
            }
        }
        if !current_scopes.is_empty() {
            raw_batches.push((current_modules, current_scopes));
        }

        raw_batches
            .into_iter()
            .enumerate()
            .map(|(index, (_modules, scopes))| ScopeBatch::from_scopes(index, scopes))
            .collect()
    }
}

fn module_of(scope: &str) -> String {
    scope
        .split_once(':')
        .map(|(module, _)| module)
        .unwrap_or("other")
        .to_owned()
}

fn fits(
    current: &BTreeSet<String>,
    additions: &[String],
    max_count: usize,
    max_encoded_bytes: usize,
) -> bool {
    let mut combined = current.clone();
    combined.extend(additions.iter().cloned());
    combined.len() <= max_count && encoded_scope_bytes(&combined) <= max_encoded_bytes
}

fn encoded_scope_bytes(scopes: &BTreeSet<String>) -> usize {
    let mut bytes = 0;
    for (index, scope) in scopes.iter().enumerate() {
        if index > 0 {
            bytes += 3; // space encoded as %20 in a query/form value
        }
        bytes += scope
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':')
                {
                    1
                } else {
                    3
                }
            })
            .sum::<usize>();
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn rejects_any_scope_outside_live_boundary() {
        let planner = ScopePlanner::new(30, 1800);
        let error = planner
            .plan(
                &set(&["docs:read"]),
                &set(&["docs:read", "mail:read"]),
                &BTreeSet::new(),
            )
            .unwrap_err();
        assert!(matches!(error, LpcError::ScopeOutOfBoundary(_)));
    }

    #[test]
    fn removes_already_effective_scopes() {
        let planner = ScopePlanner::new(30, 1800);
        let plan = planner
            .plan(
                &set(&["docs:read", "drive:read"]),
                &set(&["docs:read", "drive:read"]),
                &set(&["docs:read"]),
            )
            .unwrap();
        assert_eq!(plan.remaining, set(&["drive:read"]));
    }

    #[test]
    fn preserves_modules_when_they_fit() {
        let planner = ScopePlanner::new(3, 1800);
        let plan = planner
            .plan(
                &set(&["docs:a", "docs:b", "task:a", "task:b"]),
                &set(&["docs:a", "docs:b", "task:a", "task:b"]),
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(plan.batches.len(), 2);
        assert_eq!(plan.batches[0].modules, vec!["docs"]);
        assert_eq!(plan.batches[1].modules, vec!["task"]);
    }
}
