text_enum! {
    /// Every kind of event written to the audit log.
    ///
    /// `ReportSubmitted` is written by the `accept_report` database function,
    /// which spells out the same text in SQL; an integration test keeps the two
    /// in step.
    pub enum AuditAction {
        ReportSubmitted => "report.submitted",
        ReportUpdateIssue => "report.update_issue",
        ReportUpdateState => "report.update_state",
        SubmissionSourceUpdateState => "submission_source.update_state",
        IssueCreate => "issue.create",
        IssueUpdateState => "issue.update_state",
        IssueUpdateGithubLink => "issue.update_github_link",
        IssueSyncGithub => "issue.sync_github",
        IssueMerge => "issue.merge",
        /// Written by an earlier release; kept so those rows still display.
        IssueUpdateVisibility => "issue.update_visibility",
        SessionSignedIn => "session.signed_in",
        SessionSignedOut => "session.signed_out",
        SessionDenied => "session.denied",
        ConfigurationUpdate => "configuration.update",
        FieldDefinitionCreate => "field_definition.create",
        FieldDefinitionUpdate => "field_definition.update",
        FieldDefinitionsReorder => "field_definitions.reorder",
    }
}

/// What the entity id of an audit event refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditEntity {
    Report,
    Issue,
}

impl AuditAction {
    /// The kind of record this action's entity id points at.
    pub const fn entity(self) -> AuditEntity {
        match self {
            Self::IssueCreate
            | Self::IssueUpdateState
            | Self::IssueUpdateGithubLink
            | Self::IssueSyncGithub
            | Self::IssueMerge
            | Self::IssueUpdateVisibility => AuditEntity::Issue,
            _ => AuditEntity::Report,
        }
    }

    /// Whether the entity still has a page to open. Hiding an issue, which an
    /// earlier release did, left it without one.
    pub const fn entity_has_page(self) -> bool {
        !matches!(self, Self::IssueUpdateVisibility)
    }
}

impl AuditEntity {
    /// The entity an event refers to, given the action text stored with it.
    ///
    /// Rows written before an action was renamed or removed still exist, so
    /// text that is not a current action is classified by its prefix.
    pub fn of_stored_action(action: &str) -> Self {
        match AuditAction::parse(action) {
            Some(action) => action.entity(),
            None if action.starts_with("issue.") => Self::Issue,
            None => Self::Report,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_follow_the_action_family() {
        assert_eq!(AuditAction::IssueMerge.entity(), AuditEntity::Issue);
        assert_eq!(AuditAction::ReportUpdateState.entity(), AuditEntity::Report);
        assert_eq!(
            AuditAction::SubmissionSourceUpdateState.entity(),
            AuditEntity::Report
        );
    }

    #[test]
    fn stored_actions_from_older_releases_are_still_classified() {
        assert_eq!(
            AuditEntity::of_stored_action("issue.something_retired"),
            AuditEntity::Issue
        );
        assert_eq!(
            AuditEntity::of_stored_action("report.something_retired"),
            AuditEntity::Report
        );
        assert!(!AuditAction::IssueUpdateVisibility.entity_has_page());
        assert!(AuditAction::IssueCreate.entity_has_page());
    }

    #[test]
    fn every_issue_action_is_prefixed_like_one() {
        for action in AuditAction::ALL {
            assert_eq!(
                action.as_str().starts_with("issue."),
                action.entity() == AuditEntity::Issue,
                "{action}"
            );
        }
    }
}
