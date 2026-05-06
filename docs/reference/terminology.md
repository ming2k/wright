# Terminology

Wright uses the Ship of Theseus metaphor: the ship keeps sailing while its parts are replaced.

## Core Terms

| Term | Definition |
|------|------------|
| **Plan** | A `plan.toml` build definition. Describes how to fetch, build, and produce one or more parts. |
| **Part** | A built `.wright.tar.zst` archive. The installable unit. |
| **System** | The live machine under management, tracked in `wright.db`. |
| **Output** | A named sub-part produced by a single plan. A plan can declare multiple outputs (e.g. `gcc` and `libstdc++` from one build). |
| **Assembly** | An informal grouping of plans (a directory of plan directories) processed together by `wright apply` or `wright build`. |
| **Pack** | A `.wright.pack.tar` artifact bundling a `pack.toml` manifest, the part archives it references, and an optional `overlay/` configuration tree. The unit of distribution for a bootstrappable system. |
| **Launch** | The act of converging a target root from a pack or from plans, performed by `wright launch`. The target gets its own `wright.db` and starts coherent. |
| **Overlay** | An optional `/-rooted` tree shipped inside a pack, applied to the target after install waves. Holds base config like `/etc/hostname` and `/etc/fstab`. |

## Dependency Terms

| Term | Definition |
|------|------------|
| **`build_deps`** | Tools and headers needed during compilation. Mounted into the isolation environment at build time only. |
| **`link_deps`** | ABI-sensitive shared libraries linked by the built binary. Trigger reverse rebuilds when they change. |
| **`runtime_deps`** | Parts required for this part to function after installation. Declared per-output. Warnings issued at install time if missing. |

## Origin Values

The `origin` field on an installed part records how it entered the system.

| Origin | Meaning |
|--------|---------|
| `dependency` | Pulled in automatically to satisfy another part's dependency. Eligible for orphan cleanup via `wright remove --cascade`. |
| `build` | Installed as part of a build wave by `wright apply`. |
| `manual` | Explicitly requested by the user. Never auto-removable. |
| `external` | Declared as provided by the host system via `wright assume`. Has no filesystem footprint; used to satisfy dependency checks during bootstrap. |

Origins follow a promotion hierarchy: `dependency < build < manual`. Wright
automatically promotes an origin when you explicitly install a part that was
previously pulled in at a lower tier. `external` is managed exclusively via
`wright assume` / `wright unassume`.

## Build Terms

| Term | Definition |
|------|------------|
| **MVP build** | A reduced first-pass build that excludes certain dependencies to break a cycle. Defined by `mvp.toml` alongside `plan.toml`. |
| **Full build** | The second pass after an MVP build; runs with all dependencies restored. |
| **Lifecycle stage** | A named step in the build pipeline (e.g. `fetch`, `compile`, `staging`). Each stage runs a script in an optional isolation environment. |
| **Isolation** | A sandboxed environment for running lifecycle stages. Levels: `none`, `relaxed`, `strict`. |
| **Sysroot** | A read-only copy of the host's `/usr`, `/bin`, and `/lib` trees used as the root for `strict`-isolation builds. |

## Writing Guidance

- Say **plan** for build definitions, not "package", "formula", or "recipe".
- Say **part** for built archives, not "package" or "binary".
- Say **system** for the live machine being modified, not "host" or "target".
- Say **output** when referring to a specific named sub-part from a multi-output plan.
