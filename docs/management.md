# Management interface

## Report search

The report list updates in place after typing stops. Qualifier names and
low-cardinality values are suggested at the caret. Fifty reports are loaded at
a time, and **Show more…** appends the next page without navigating away.
Plain words search report identifiers,
types, client versions, build descriptions, and all submitted field values.
Qualifiers restrict a value to one property:

```text
platform:linux kind:crash
version:"Ladybird Nightly" architecture:arm64
state:confirmed signal:sigabrt
```

Built-in qualifiers are `state`, `kind`, `version`, `client_version`, `build`,
`id`, `report`, `ip`, and `source_ip`. The `state` value is `triage`,
`confirmed`, `assigned`, or `all`. Any other qualifier is treated as a
submitted field key. Values containing spaces can be quoted.

Report metadata and single-line diagnostic fields provide filter buttons that
construct the corresponding qualified search.

Confirming a report moves it from triage to the confirmed state. Deleting a
report hides it from the management interface while retention continues to
govern when its stored data is removed.

Blocking a submission source rejects its future public API requests. The
confirmation also offers to hide every report from that source that is still
in triage; confirmed and assigned reports remain available.

The issue workflow opens from a report. It can search active internal issues and
add the report to one, or create an internal issue for the report. A new internal
issue may remain local, link to an existing GitHub issue, or create a GitHub
issue. GitHub search results identify issues already tracked internally so the
existing internal issue is reused. The resolved-issue filter applies as soon as
it changes.

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
changes, field definition changes, source blocks, and issue actions. Anonymous
report submission events have no maintainer actor.
