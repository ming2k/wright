# Repositories

Wright resolves parts by name from configured **sources** — directories
containing `.wright.tar.zst` archives. Repository metadata is stored in
SQLite at `/var/lib/wright/db/repo.db` by default.

Repository management is handled by the dedicated `wrepo` tool.

## Concepts

| Term | What it is |
|------|-----------|
| **Source** | A configured directory (local) or URL (remote, future) where wright looks for parts |
| **Repo DB** | The SQLite catalog (`repo.db`) populated from part `.PARTINFO` metadata |
| **Resolver** | The component that finds a part archive by name — checks the repo DB first, falls back to scanning archives |

## Quick Start

The default local repository is `components_dir` (`/var/lib/wright/components`
by default). Parts built by `wbuild` are placed there automatically, so the
simplest workflow is:

```bash
# 1. Build parts (output goes to components_dir)
wbuild run curl

# 2. Sync the default repo into SQLite
wrepo sync

# 3. Install by name
wright install curl
```

For a custom repo directory, specify the path explicitly:

```bash
mkdir -p /var/lib/wright/myrepo
cp *.wright.tar.zst /var/lib/wright/myrepo/
wrepo sync /var/lib/wright/myrepo
wrepo source add myrepo --path=/var/lib/wright/myrepo
wright install curl
```

## Managing Sources

Sources are stored in `/etc/wright/repos.toml`. Use the `wrepo source`
commands to manage them without editing the file by hand.

### Add a source

```bash
wrepo source add myrepo --path=/var/lib/wright/myrepo
wrepo source add myrepo --path=/var/lib/wright/myrepo --priority=300
```

| Flag | Default | Description |
|------|---------|-------------|
| `--path=<PATH>` | *(required)* | Local directory path |
| `--priority=<N>` | `100` | Higher number = preferred when the same part exists in multiple sources |

### List sources

```bash
wrepo source list
# myrepo          local    pri=300  /var/lib/wright/myrepo
```

### Remove a source

```bash
wrepo source remove myrepo
```

## Indexing

`wrepo sync` imports `.wright.tar.zst` metadata into the repo DB. `wbuild`
already registers newly built parts there directly, so `sync` is mainly for
existing or externally copied archives.

```bash
wrepo sync                          # index the default components_dir
wrepo sync ./components             # index a specific directory
```

**Re-run `wrepo sync` whenever you add or update archives outside `wbuild`.**

The repo DB records for each part:
- Name, version, release, epoch, architecture
- Description
- Runtime dependencies
- Provides, conflicts, replaces
- Filename and SHA-256 checksum

A single part name can have multiple versions in the repo DB. The resolver
collects all versions and picks the latest (or a user-specified version).

## Listing and Searching

```bash
wrepo list                   # all indexed parts
wrepo list zlib              # all available versions of zlib
wrepo search zlib            # search by keyword (name + description)
wrepo search ssl             # search by keyword (name + description)
```

Output marks the currently installed version with `[installed]`:

```
gcc 15.1.0-1 (x86_64) [installed]
gcc 14.2.0-3 (x86_64)
gcc 14.2.0-2 (x86_64)
```

## Removing Parts from the Repository

```bash
wrepo remove zlib 1.3.1             # remove DB entry only
wrepo remove zlib 1.3.1-2 --purge   # also delete the archive file
```

## Upgrading

With an indexed repository, upgrades work by name:

```bash
wright upgrade gcc                       # upgrade to the latest available version
wright upgrade gcc --version=14.2.0      # switch to a specific version
wright sysupgrade                        # upgrade all installed parts to latest
wright sysupgrade -n                     # dry-run: preview without changes
```

The resolver finds all available versions across configured sources and picks
the latest (or the version specified with `--version`). When `--version` is
given, `--force` is implied so downgrades work without an extra flag.

## Multiple Repos

When the same part exists in multiple sources, the source with the
higher `priority` wins. This lets you layer a local build repo on top of
a shared team repo:

```bash
wrepo source add team   --path=/mnt/shared/wright-repo --priority=100
wrepo source add local  --path=/var/lib/wright/myrepo   --priority=300
```

Parts you build locally (priority 300) shadow the team repo (priority 100).

## Typical Workflows

### Personal build-and-install

```bash
wbuild run -i curl              # build and install in one step (no index needed)
```

### Build, index, upgrade

```bash
wbuild run gcc                  # build updated gcc
wrepo sync                      # re-index
wright upgrade gcc              # upgrade to the newly built version
```

### Shared team repo

```bash
# Builder machine
wbuild run @core
wrepo sync

# Developer machines
wrepo source add builds --path=/mnt/nfs/wright-components
wright install @base
```

### Assembly build with repo

```bash
wbuild run -i --deps=sync @qemu # build and install, syncing missing/outdated deps
wrepo sync                      # update the index
```

## Tool Reference

| Tool | Role |
|------|------|
| `wright` | System administrator — install, remove, upgrade, query |
| `wbuild` | Part builder — build from plan.toml |
| `wrepo` | Repository manager — index, search, source management |
