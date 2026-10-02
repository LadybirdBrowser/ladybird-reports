text_enum! {
    /// Where a report is in triage. Mirrors the `reports_state_valid` constraint.
    pub enum ReportState {
        Triage => "triage",
        Confirmed => "confirmed",
        Rejected => "rejected",
    }
}

text_enum! {
    /// Where an internal issue is in its life. Mirrors the `issues_state_valid` constraint.
    pub enum IssueState {
        Unresolved => "unresolved",
        NeedsAttention => "needs_attention",
        Resolved => "resolved",
        Rejected => "rejected",
    }
}

impl IssueState {
    /// Whether the issue still needs work.
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Unresolved | Self::NeedsAttention)
    }

    /// The state an issue takes from what is known about its GitHub issue: a
    /// closed one is resolved, one that cannot be read needs a person to look,
    /// anything else is still open.
    pub const fn tracking(link: GithubLinkState) -> Self {
        match link {
            GithubLinkState::Closed => Self::Resolved,
            GithubLinkState::Missing | GithubLinkState::Moved | GithubLinkState::Unavailable => {
                Self::NeedsAttention
            }
            GithubLinkState::Unknown | GithubLinkState::Open => Self::Unresolved,
        }
    }
}

text_enum! {
    /// What is known about the GitHub issue an internal issue tracks. Mirrors the
    /// `issues_github_state_check` constraint. `Open` and `Closed` come from GitHub;
    /// the rest describe this service losing track of the issue.
    pub enum GithubLinkState {
        Unknown => "unknown",
        Open => "open",
        Closed => "closed",
        Missing => "missing",
        Moved => "moved",
        Unavailable => "unavailable",
    }
}

impl GithubLinkState {
    /// The linked issue cannot be read from where it was linked.
    pub const fn is_unreachable(self) -> bool {
        matches!(self, Self::Missing | Self::Moved | Self::Unavailable)
    }

    /// The issue is known to be deleted or transferred, not just unreadable.
    pub const fn is_gone(self) -> bool {
        matches!(self, Self::Missing | Self::Moved)
    }
}

text_enum! {
    /// What rejecting an issue does to the reports assigned to it.
    pub enum IssueReportAction {
        Unlink => "unlink",
        Reject => "reject",
    }
}

text_enum! {
    /// What caused an internal issue to be brought in line with its GitHub issue.
    pub enum GithubSyncSource {
        Webhook => "webhook",
        Assignment => "assignment",
    }
}

text_enum! {
    /// Whether a report's attachment files have been stored. Mirrors the
    /// `reports_storage_state_check` constraint.
    pub enum StorageState {
        PendingFiles => "pending_files",
        Ready => "ready",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_state_decides_the_issue_state() {
        assert_eq!(
            IssueState::tracking(GithubLinkState::Open),
            IssueState::Unresolved
        );
        assert_eq!(
            IssueState::tracking(GithubLinkState::Unknown),
            IssueState::Unresolved
        );
        assert_eq!(
            IssueState::tracking(GithubLinkState::Closed),
            IssueState::Resolved
        );

        for state in GithubLinkState::ALL {
            assert_eq!(
                IssueState::tracking(*state) == IssueState::NeedsAttention,
                state.is_unreachable(),
                "{state}"
            );
        }
    }

    #[test]
    fn open_issues_are_the_ones_that_need_work() {
        assert!(IssueState::Unresolved.is_open());
        assert!(IssueState::NeedsAttention.is_open());
        assert!(!IssueState::Resolved.is_open());
        assert!(!IssueState::Rejected.is_open());
    }
}
