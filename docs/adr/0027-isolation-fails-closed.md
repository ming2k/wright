# ADR-0027: Isolation fails closed

## Status

Accepted

## Context

Build plans execute code obtained from package sources. The `relaxed` and
`strict` isolation levels promise Linux namespace boundaries, but the runner
previously fell back to direct host execution when the required namespaces
were unavailable. A warning did not make that privilege change explicit, and
the fallback was especially risky because `strict` is the default.

The runner also skipped the user namespace when Wright was already running as
root. A mount namespace created from the initial user namespace does not
remove root's capabilities over the host. Linux user namespaces instead give
the process capabilities in the child namespace without retaining
capabilities in the parent namespace.

## Decision

`none` is the only isolation level that executes directly on the host.

Both `relaxed` and `strict` require all of their declared namespaces,
including a user namespace regardless of Wright's effective user ID. If the
kernel or execution environment denies any required namespace, Wright stops
before executing the build command and reports how to opt into `none`
explicitly.

Before executing an isolated command, Wright sets `no_new_privs`, supplies an
explicit minimal environment, and requires an absolute executable path.
Isolation paths and mount targets are validated before the runner forks.

## Alternatives

### Continue with a warning and direct execution

This preserves compatibility on hosts that disable user namespaces, but it
silently changes the security boundary of a plan. A warning is insufficient
for code execution on the host.

### Fall back only for `relaxed`

`relaxed` shares network and IPC state, but it still promises mount, PID, UTS,
and user namespace boundaries. Direct execution would violate those semantics.

### Skip the user namespace for root

This improves compatibility with kernels that restrict user namespaces, but
leaves the isolated command with capabilities in the initial user namespace.
Users that intentionally need host execution can select `none`.

## Consequences

- A requested isolation level never becomes host execution implicitly.
- Root and non-root invocations use the same capability boundary.
- Hosts that disable user namespaces must enable them or select `none`
  explicitly.
- Absolute executor paths are required for isolated stages.
- Namespace availability failures become deterministic errors rather than
  warnings.
