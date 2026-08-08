# ADR-0031: Folio Manifest Amendments

## Status

Accepted

## Context

[ADR-0015](0015-folio-manifest-replaces-pack.md) replaced the pack format
with the folio manifest. As implemented there, a folio was a fixed-name
`folio.toml` carrying an `arch` field and a `[config]` table (hostname,
timezone, locale, services), and folio references were consumed by the
`wright apply` command.

Four of those details have since changed in code:

1. **The `[config]` table was removed.** Declarative system configuration
   made Wright act as a configuration-management system, which is outside
   its mission as a plan combinator. Post-launch configuration is expressed
   as `[[hook]]` scripts (`stage = "post-launch"`) instead.
2. **`arch` was removed from folio meta.** It was informational only and had
   no behavioral effect.
3. **The file convention changed.** Folios are bare TOML files named after
   the folio (`<name>.toml`, e.g. `core.toml`) living in a folios directory
   that is a peer of the plans directory (`general.folios_dir`, default
   `/var/lib/wright/folios`, overridable per invocation with
   `wright launch --folios <DIR>`). The fixed `folio.toml` name and the
   `<plans_dir>/folios/` discovery fallback are gone.
4. **`apply` was renamed to `install`.** `@name` folio references are now
   accepted by `wright install` and `wright launch`; there is no
   `wright apply` command.

The parser rejects unknown keys and tables (`deny_unknown_fields`), so old
folios carrying `[config]` or `arch` fail at parse time rather than being
silently misread.

## Decision

The folio manifest schema is exactly: a required `[folio]` table (`name`,
`version`, optional `description`, optional `plans` list), optional
repeatable `[[provide]]` entries (`name`, `version`), and optional
repeatable `[[hook]]` entries (`stage`, `script`; only `"post-launch"` is
recognized). Files are named `<name>.toml` and resolved from the folios
search dirs only.

This ADR supersedes the manifest-format details of
[ADR-0015](0015-folio-manifest-replaces-pack.md). ADR-0015's core ruling —
a pure plan-list manifest replaces the binary pack format — remains in
force.

## Alternatives

### Keep `[config]` and grow it carefully

Any declarative config table invites feature creep toward configuration
management. Hooks already cover the same cases with no new schema. Rejected.

### Keep `arch` for documentation value

A field nothing reads is a field that lies. Plan metadata already records
the target architecture. Rejected.

### Amend ADR-0015 in place

ADRs are immutable once accepted; amendments are recorded as a new ADR per
the project's ADR workflow. Rejected.

## Consequences

- Migration for old folios: move every `[config]` setting into a
  `[[hook]]` post-launch script, delete `arch`, and rename the file to
  `<name>.toml` inside `folios_dir`.
- One folio directory can hold many named manifests, so systems compose:
  `wright launch --root /mnt/new @base @desktop`.
- Post-launch behavior is init-system-agnostic; the folio author controls it
  through hook scripts, which run unsandboxed on the host.
