# Management interface

## Report search

The report list has one search field. Plain words search report identifiers,
types, client versions, build descriptions, and all submitted field values.
Qualifiers restrict a value to one property:

```text
platform:linux kind:crash
version:"Ladybird Nightly" architecture:arm64
state:assigned signal:sigabrt
```

Built-in qualifiers are `state`, `kind`, `version`, `client_version`, `build`,
`id`, `report`, `ip`, and `source_ip`. The `state` value is `triage`, `assigned`,
or `all`. Any other qualifier is treated as a submitted field key. Values
containing spaces can be quoted.

Report metadata and single-line diagnostic fields provide filter buttons that
construct the corresponding qualified search.

Blocking a submission source rejects its future public API requests. The
confirmation also offers to remove every report from that source that is still
in triage; assigned reports remain available.

## Runtime settings

Runtime configuration is stored as one active JSON document. Saving replaces
that document after validating it against compiled safety limits. The setting
details panel describes the setting on the editor line containing the caret.

Recognized field definitions control labels, value types, and display order.
Drag a row handle to reorder it; the new order is saved immediately. Unknown
fields remain available in reports until a definition is added.

## Audit log

The audit log records report submissions, sign-ins and sign-outs, configuration
changes, field definition changes, source blocks, and issue actions. Anonymous
report submission events have no maintainer actor.
