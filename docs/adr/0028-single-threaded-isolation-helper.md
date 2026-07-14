# ADR-0028: Namespace setup runs in a single-threaded helper process

## Status

Accepted

## Context

Wright starts build stages from a Tokio application that may have multiple
worker and output-capture threads. Linux PID namespace setup requires a fork
before the namespace init process starts. The previous implementation
performed allocation, filesystem traversal, namespace setup, and mount
operations in the child of that multithreaded application process.

After a multithreaded process forks, library locks held by vanished threads
can remain locked in the child. Restricting the child to async-signal-safe
operations until `exec` would prevent Wright from performing the required
namespace and mount setup there.

## Decision

The application process starts a fresh copy of the current `wright`
executable for every `relaxed` or `strict` stage. The executable detects an
internal, versioned helper invocation before creating a Tokio runtime. This
fresh process is single-threaded when it validates the request and performs
the fork, namespace, mount, and root-pivot sequence.

The application sends a versioned request over standard input. Paths retain
their original operating-system byte representation. A separate private
status file distinguishes helper setup failures from stage exit codes, while
standard output and standard error remain streaming pipes. Timeout and
cancellation supervision stay in the application process and terminate the
helper, whose parent-death chain tears down the PID namespace.

The helper protocol is internal. It is not a compatibility surface for
third-party tools. The normal installation continues to contain one `wright`
binary.

## Alternatives

### Continue forking in the application process

This avoids a process hop, but complex post-fork Rust and libc activity can
deadlock on state inherited from other threads. The risk increases as the
application gains more concurrent services.

### Install a separate helper executable

A dedicated executable makes the boundary visible, but introduces packaging,
version-skew, and lookup failures. Re-entering the same binary guarantees that
the application and protocol implementation have the same version.

### Extract the current module into a crate

A crate creates a compile-time dependency boundary but does not change the
unsafe process context in which namespace setup runs. A crate may follow once
the helper-facing configuration and error API is stable.

## Consequences

- Complex namespace setup no longer runs in a child forked from the
  multithreaded application process.
- Helper lookup, request decoding, and status handling fail closed before a
  stage can run without its requested isolation.
- Stage output remains streamed and separated, and stage exit signals remain
  observable by the application process.
- Embedders whose executable does not provide Wright's internal entry point
  must supply a compatible helper executable.
- Each isolated stage adds one short-lived supervisor process and a small
  request-serialization cost.
