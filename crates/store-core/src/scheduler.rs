use crate::{OperationAction, SourceKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LockDomain {
    Network,
    AptLists,
    DpkgDatabase,
    FlatpakUser,
    FlatpakSystem,
    GithubInstallPrefix,
    PrivilegePrompt,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LockPlan {
    pub shared: Vec<LockDomain>,
    pub exclusive: Vec<LockDomain>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SchedulerLimits {
    pub max_active_operations: usize,
    pub max_network_downloads: usize,
    pub max_github_jobs: usize,
    pub max_flatpak_mutations: usize,
    pub max_apt_mutations: usize,
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            max_active_operations: 4,
            max_network_downloads: 4,
            max_github_jobs: 2,
            max_flatpak_mutations: 1,
            max_apt_mutations: 1,
        }
    }
}

impl LockPlan {
    pub fn for_operation(source: SourceKind, action: OperationAction) -> Self {
        let mut shared = Vec::new();
        let mut exclusive = Vec::new();

        if matches!(
            action,
            OperationAction::Install | OperationAction::Update | OperationAction::Reinstall
        ) {
            shared.push(LockDomain::Network);
        }

        match source {
            SourceKind::System => {
                exclusive.push(LockDomain::DpkgDatabase);
                exclusive.push(LockDomain::PrivilegePrompt);
            }
            SourceKind::Flathub => exclusive.push(LockDomain::FlatpakUser),
            SourceKind::Github => {
                shared.push(LockDomain::Network);
                exclusive.push(LockDomain::GithubInstallPrefix);
            }
            SourceKind::Appimage => exclusive.push(LockDomain::GithubInstallPrefix),
        }

        Self { shared, exclusive }
    }

    pub fn conflicts_with(&self, other: &LockPlan) -> bool {
        self.exclusive
            .iter()
            .any(|domain| other.exclusive.contains(domain) || other.shared.contains(domain))
            || other
                .exclusive
                .iter()
                .any(|domain| self.shared.contains(domain))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apt_mutations_conflict() {
        let a = LockPlan::for_operation(SourceKind::System, OperationAction::Install);
        let b = LockPlan::for_operation(SourceKind::System, OperationAction::Remove);
        assert!(a.conflicts_with(&b));
    }

    #[test]
    fn system_and_flathub_can_run_together() {
        let a = LockPlan::for_operation(SourceKind::System, OperationAction::Install);
        let b = LockPlan::for_operation(SourceKind::Flathub, OperationAction::Install);
        assert!(!a.conflicts_with(&b));
    }
}
