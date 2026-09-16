# Management interface

## Report search

The report list updates in place when the search field loses focus or Enter is
pressed. Qualifier names and low-cardinality values are suggested at the caret.
Fifty reports are loaded at a time, and **Show more…** appends the next page
without navigating away.
Plain words search report identifiers,
types, client versions, build descriptions, and all submitted field values.
Qualifiers restrict a value to one property:

```text
platform:linux kind:crash
version:"Ladybird Nightly" architecture:arm64
state:triage state:confirmed signal:sigabrt
```

Built-in qualifiers are `state`, `kind`, `version`, `client_version`, `build`,
`id`, `report`, `ip`, and `source_ip`. State values are `triage`, `confirmed`,
and `assigned`, or `all`. Repeating a qualifier matches any of its values, so
`state:triage state:confirmed` includes both states. Different qualifiers are
combined, so adding `platform:linux` restricts both states to Linux reports.
The default is `state:triage state:confirmed`. Any other qualifier is treated
as a submitted field key. Values containing spaces can be quoted.

Report metadata and single-line diagnostic fields provide filter buttons that
construct the corresponding qualified search.

The state selector moves an unassigned report between triage and confirmed.
Deleting a report hides it from the management interface while retention
continues to govern when its stored data is removed.

Blocking a submission source rejects its future public API requests. The
confirmation also offers to hide every report from that source that is still
in triage; confirmed and assigned reports remain available.

The issue workflow opens from a report. Its combined search shows active issues
already tracked in the service and matching issues from the configured GitHub
repository. A GitHub issue already tracked in the service appears only once.
Selecting an untracked GitHub issue creates the corresponding issue record and
adds the report. Creating an issue publishes it to GitHub and records it in the
service as one operation. The editable draft uses the report type and selected
environment fields, while omitting URLs, stack traces, attachments, and unknown
fields. Every issue in the service therefore has a GitHub issue at creation.
The resolved-issue filter applies as soon as it changes. GitHub controls the
issue's title, description, and open or closed state. Signed GitHub webhooks
update those cached values in Reports. Report assignments remain intact when
an issue is closed, deleted, or transferred. A deleted or moved link is flagged for
repair, and its previous GitHub identity stays associated with the issue.
The report view links to its tracked issue. From the issue view, a maintainer
can unlink individual reports. Deleting an issue hides its Reports record and
unlinks all remaining reports, which return to triage or confirmed. Neither
action changes the GitHub issue, and a hidden issue cannot receive reports.
Merging two tracked issues moves the reports to the destination. Maintainers
update the source GitHub issue separately.
When `github_reports_issue_field_id` is configured, opening a tracked issue
also ensures its organization-only GitHub sidebar field links back to Reports.
The field is set for newly created and existing tracked issues.

## Runtime settings

Runtime configuration is stored as one active JSON document. Saving replaces
that document after validating it against compiled safety limits. The setting
details panel describes the setting on the editor line containing the caret.

The `discord` object controls report notifications. Set `webhook_url` to a Discord
incoming-webhook URL to enable delivery, or to `null` to pause it without discarding
queued notifications. The remaining values control queue polling, request timeouts,
retry delays, and the native-stack excerpt included in each message.

Recognized field definitions control labels, value types, and display order.
Drag a row handle to reorder it; the new order is saved immediately. Unknown
fields remain available in reports until a definition is added.

## Audit log

The audit log records report submissions, sign-ins and sign-outs, configuration
changes, field definition changes, source blocks, and issue actions. Action
names describe the operation; details record the affected fields or state
transition. For example, `report.update_state` includes `from` and `to` values.
Configuration events record changed setting paths without storing their values
again. Anonymous report submission events have no maintainer actor.
