# Terminology

Wright uses a deliberately unified metaphor: treat the computer as the **Ship of Theseus** while it is sailing.

The project is not just a system manager in the narrow sense. It is a language and toolchain for planning, building, storing, and replacing the ship's parts without losing control of the vessel as a whole. The terminology is meant to make those roles visually distinct.

## Core Metaphor

- A **system** is the ship currently in service.
- A **plan** is the blueprint for making one replacement part.
- A **part** is the finished, installable replacement piece.
- A **repository** is the harbor inventory of finished parts.
- An **assembly** is a build-time bundle of blueprints.
- A **kit** is an install-time bundle of finished parts.

The important distinction is that Wright names things by their role in the pipeline, not by vague reuse of one generic word everywhere.

## Canonical Terms

### Plan

A **plan** is a `plan.toml` file plus its directory context.

It describes how to produce one part:

- identity: name, version, release, architecture
- sources: where inputs come from
- dependencies: what must exist to build or run
- lifecycle: how the build proceeds stage by stage
- options and metadata: validation, compatibility, and policy hints

In plain terms: a plan is the recipe, drawing, and work order for manufacturing one replacement piece of the ship.

### Part

A **part** is the finished output built from a plan.

Usually this means a `.wright.tar.zst` archive with embedded metadata and payload files ready to install onto a system.

In plain terms: a part is the actual timber, sail, mast, or fitting that can be loaded onto the ship and put into service.

Wright prefers **part** when talking about the artifact itself, because the project wants a stronger distinction between:

- the description of how to build something
- the finished binary artifact
- the act of installing it onto a live system

### Repository

A **repository** is a catalog of finished parts, not of plans.

It is where `wrepo` indexes built archives so that `wright` can resolve names, versions, and sources when installing or upgrading.

In plain terms: the repository is the harbor storehouse of ready-made replacement parts.

### Source

A **source** is a location Wright can read from during either build resolution or binary resolution.

Depending on context, a source may refer to:

- upstream source tarballs used by a plan during build
- a configured repository source used by `wright` and `wrepo`

The word stays the same because both cases answer the same question: "where does this input come from?" The surrounding context should make clear whether the input is raw build material or finished binary stock.

### Assembly

An **assembly** is a named build-time grouping of plans.

Assemblies are consumed by `wbuild`. They are combinatory convenience groups, not dependency graphs by themselves.

In plain terms: an assembly is a set of blueprints you want the shipyard to work through together.

### Kit

A **kit** is a named install-time grouping of parts.

Kits are consumed by `wright install` through `@kit` references. Like assemblies, they are convenience groupings rather than dependency declarations.

In plain terms: a kit is a crate of replacement parts commonly loaded onto ships together.

### System

The **system** is the live machine under management: the currently sailing Ship of Theseus.

`wright` operates on this level. It decides which parts are installed, upgraded, removed, assumed external, verified, or diagnosed.

## Tool Vocabulary

Each binary owns one stage of the metaphor:

| Tool | Owns | Primary nouns |
|------|------|---------------|
| `wbuild` | manufacture | plan, source, lifecycle, assembly, part |
| `wrepo` | catalog and supply | repository, source, part |
| `wright` | live vessel maintenance | system, installed part, kit, dependency, history |

This is why the project is split into three binaries: building a part, cataloging a part, and installing a part are related, but they are not the same operation.

## Writing Guidance

When writing new docs, help text, or code comments, prefer these distinctions:

- Say **plan** when you mean `plan.toml` and its build definition.
- Say **part** when you mean the built `.wright.tar.zst` artifact.
- Say **assembly** for build-time groups of plans.
- Say **kit** for install-time groups of parts.
- Say **system** for the live machine being modified.

If a sentence becomes ambiguous, rewrite it using the more specific term instead.

## Preferred Usage Matrix

| Context | Preferred term | Notes |
|------|------|------|
| Build definition | **plan** | `plan.toml`, lifecycle, sources, dependencies |
| Built archive | **part** | `.wright.tar.zst`, `.PARTINFO`, repository entry |
| Install-time bundle | **kit** | `wright install @base` |
| Build-time bundle | **assembly** | `wbuild run @core` |
| Live machine state | **system** | installed parts, health checks, transactions |
| Generic CLI placeholder | **part** | Prefer `<PART>` in help and examples |
| Internal DB / code names | **part** | Keep implementation terms aligned with the conceptual model when practical |

## Implementation Naming

Wright now tries to keep implementation names aligned with the glossary as well:

- SQL tables such as `parts`
- fields such as `part_id`
- archive metadata sections such as `[part]`

User-facing writing should still prefer the conceptual terms from this glossary, but the implementation is no longer treated as a separate legacy vocabulary.

## One-Line Summary

Wright plans parts, builds parts, catalogs parts, and swaps parts on a living ship without losing track of the ship itself.
