# Terminology

Wright uses the Ship of Theseus metaphor while the ship is still sailing.

## Core Terms

- **Plan**: a `plan.toml` build definition.
- **Part**: a built `.wright.tar.zst` archive.
- **Assembly**: a build-time grouping of plans.
- **Archive Catalogue**: the local database of already built archives on this machine (managed via `archives.db`).
- **System**: the live machine under management.

## Writing Guidance

- Say **plan** for build definitions.
- Say **part** for built archives.
- Say **assembly** for grouped build targets.
- Say **catalogue** or **archive database** for the local built-archive inventory.
- Say **system** for the live machine being modified.

Wright intentionally does not reuse one vague word for all four layers.
