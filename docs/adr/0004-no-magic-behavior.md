# ADR-0004: No Implicit Magic Behavior

## Status

Accepted

## Context

Wright targets LFS-based distributions where users are power users who expect predictable, auditable behavior. There is tension between:

1. **Convenience**: Implicit automation that saves keystrokes.
2. **Explicitness**: Every action is visible in the plan.

## Decision

Wright does not perform implicit actions on behalf of the plan author. If the tool does something, it must be because the plan explicitly asked for it.

**Concrete example — patch application.** A plan that needs patches declares them as `[[sources]]` entries and applies them explicitly in the `prepare` script. Wright will never auto-detect `.patch` files and apply them silently.

When evaluating a feature request, ask: does this save meaningful work, or does it only save the user from writing something explicit and readable? If the latter, prefer keeping behavior explicit.

## Consequences

- Plans are self-contained and readable.
- No hidden conventions that must be memorized.
- Edge cases are reduced because there are fewer implicit rules.
- Slightly more verbose plans, but greater transparency.
