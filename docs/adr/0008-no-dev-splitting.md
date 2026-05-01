# ADR-0008: No -dev Splitting for Personal Distributions

## Status

Accepted

## Context

Traditional distributions (Debian, Alpine) split headers, static libraries, and pkg-config files into `-dev` sub-parts. This saves disk space but increases complexity.

Wright's target users are personal or small-team custom distributions.

## Decision

Do **not** split `-dev` sub-parts by default. Keep headers, `.a`, and `.pc` files in the main part.

**Exception**: If development files are exceptionally large (e.g., Qt, LLVM headers > 50MB), consider splitting.

## Consequences

- **Lower maintenance cost**: No extra dependency declarations, version tracking, or testing for `-dev` packages.
- **Build-friendly**: Installing a library immediately allows compiling software that depends on it.
- **Debug-friendly**: Headers are available during troubleshooting.
- **Negligible disk overhead**: Headers and `.pc` files typically take only a few hundred KB.
