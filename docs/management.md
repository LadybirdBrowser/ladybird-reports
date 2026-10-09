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
`id` and `report`. State values are `triage`, `confirmed`,
and `rejected`, or `all`. Repeating a qualifier matches any of its values, so
`state:triage state:confirmed` includes both states. Different qualifiers are
combined, so adding `platform:linux` restricts both states to Linux reports.
The default is `state:triage state:confirmed`. Any other qualifier is treated
as a submitted field key. Values containing spaces can be quoted.

Report metadata and single-line diagnostic fields provide filter buttons that
construct the corresponding qualified search.

Stack trace fields render as a frame table with the exact original text available
below it. Reports stores that text unchanged. A report is signed when it
is accepted, and a background job signs existing reports and regenerates old
signatures when the algorithm version changes. A new report whose signature
exactly matches one active issue is linked to it automatically; other possible
matches appear on unassigned report pages for maintainers to review and link
manually. Signature data is removed when its report is purged.

The report list folds reports with the same stack signature into one row, with
the number of reports it holds. The row is placed where the most recent of them is,
and opens to show them. Groups are made in the browser from the reports that are
loaded, so a group grows as more reports are loaded, and nothing is stored.

Report actions can confirm a report, return an unlinked report to triage, or
reject a report with a confirmation dialog. Rejected reports remain available through
`state:rejected`; retention still governs when their data is removed. Adding
a report to an issue confirms it.

Blocking a submission source rejects its future public API requests and
rejects the selected report. The confirmation also offers to reject other
reports from that source that are still in triage.

The issue workflow opens from a report. Its combined search shows active issues
already tracked in the service and matching issues from the configured GitHub
repository. A GitHub issue already tracked in the service appears only once.
Selecting an untracked GitHub issue creates the corresponding issue record and
adds the report. Creating an issue publishes it to GitHub and records it in the
service as one operation. The editable draft uses the report type, selected
environment fields, and the raw stack trace in a code block. It omits URL fields,
attachments, and unknown fields. Maintainers can review and edit the public
draft before creating it. Every issue in the service has a GitHub issue at creation.
The issue list uses the same search interaction as the report list: Enter or
leaving the field updates results without reloading the page. Its default
query is `state:unresolved state:needs_attention`; clearing the query shows
all states. Plain words search titles, issue IDs, and GitHub issue numbers.
Use `state:resolved`, `state:rejected`, `github:4812`, or an `id:` prefix to
narrow results. Repeating `state:` includes either state, and GitHub-number
search also recognizes previous links kept as aliases.

GitHub controls the
issue's title, description, and open or closed state. Signed GitHub webhooks
update those cached values in Reports. Report assignments remain intact when
an issue is closed, deleted, or transferred. A deleted or moved link is flagged for
repair, and its previous GitHub identity stays associated with the issue.
The report view links to its tracked issue. From the issue view, a maintainer
can unlink individual reports. Deleting an issue hides its Reports record and
unlinks all remaining reports without changing their state. Neither
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

The `audit` object controls audit log behavior. `denied_sign_in_window_seconds`
(default 600) limits how often one GitHub account that is denied at sign-in is
recorded; set it to 0 to record every denial.

`trusted_proxies` lists the proxies whose forwarding headers identify the original
client address. Each entry is a CIDR range or a DNS name. The public API resolves
names itself and trusts only the addresses it received, never the name. It uses a
resolution for at most five minutes, so a proxy that gets a new address is trusted
at that address within that time. If a name cannot be resolved again, the previous
addresses stay in use and a warning is logged. The public API refuses to start when
a name does not resolve at all. Deploy a release that understands names before
saving one, because an older public API cannot read the configuration afterwards.

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

`session.denied` records a GitHub account that completed sign-in but is not on
the authorization team. The account is not a maintainer, so its login is shown in
the actor column and its GitHub ID is in the details. Repeated denials from the
same account within `audit.denied_sign_in_window_seconds` (default 600) are
recorded once; set it to 0 to record every denial. GitHub outages and rate limits
during sign-in are not recorded as denials.
