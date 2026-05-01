# ADR-0007: usrmerge and sbin Merged into bin

## Status

Accepted

## Context

The target distribution follows a streamlined FHS variant optimized for musl + runit. Historical Unix splits (`/bin` vs `/usr/bin`, `/sbin` vs `/usr/sbin`) create complexity without benefit.

## Decision

- `/bin`, `/sbin`, and `/lib` are all symlinks to their counterparts under `/usr/`.
- `/usr/sbin` does not exist; root-only tools live in `/usr/bin`.
- Privilege is enforced by permissions, not path.

## Consequences

- All parts must install files under `/usr/`.
- No separate `sbin` directory simplifies packaging.
- `lib64` handling varies by architecture: on musl it symlinks to `/usr/lib`; on glibc multi-arch it may point to `/usr/lib64`.
- `/usr/local/` is reserved for manual user installs; Wright-managed parts must never install there.
