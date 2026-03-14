# Repositories

Wright resolves parts by name from configured **sources** — directories
containing `.wright.tar.zst` archives and an index file.

Repository management is handled by the dedicated `wrepo` tool.

## Concepts

| Term | What it is |
|------|-----------|
| **Source** | A configured directory (local) or URL (remote, future) where wright looks for parts |
| **Index** | A `wright.index.toml` file listing every part in a source with its metadata and checksum |
| **Resolver** | The component that finds a part archive by name — checks the index first, falls back to scanning archives |

## Quick Start

The default local repository is `components_dir` (`/var/lib/wright/components`
by default). Packages built by `wbuild` are placed there automatically, so the
simplest workflow is:

```bash
# 1. Build packages (output goes to components_dir)
wbuild run curl

# 2. Index the default repo
wrepo sync

# 3. Install by name
wright install curl
```

For a custom repo directory, specify the path explicitly:

```bash
mkdir -p /var/lib/wright/myrepo
cp *.wright.tar.zst /var/lib/wright/myrepo/
wrepo sync /var/lib/wright/myrepo
wrepo source add myrepo --path /var/lib/wright/myrepo
wright install curl
```

## Managing Sources

Sources are stored in `/etc/wright/repos.toml`. Use the `wrepo source`
commands to manage them without editing the file by hand.

### Add a source

```bash
wrepo source add myrepo --path /var/lib/wright/myrepo
wrepo source add myrepo --path /var/lib/wright/myrepo --priority 300
wrepo source add holdtree --type hold --path /var/lib/wright/plans
```

If you keep plans in a user-owned tree instead of the system default, point
the hold source at that directory instead, for example
`--path ~/wright/plans`.

| Flag | Default | Description |
|------|---------|-------------|
| `--path <PATH>` | *(required)* | Local directory path |
| `--type <TYPE>` | `local` | `local` (binary packages) or `hold` (plan source tree) |
| `--priority <N>` | `100` | Higher number = preferred when the same package exists in multiple sources |

### List sources

```bash
wrepo source list
# myrepo          local    pri=300  /var/lib/wright/myrepo
# holdtree        hold     pri=100  /var/lib/wright/plans
```

### Remove a source

```bash
wrepo source remove myrepo
```

## Indexing

The index (`wright.index.toml`) is what makes name-based resolution fast.
Without it, the resolver must decompress and read `.PARTINFO` from every
archive in the directory.

```bash
wrepo sync                          # index the default components_dir
wrepo sync /var/lib/wright/myrepo   # index a specific directory
```

**Re-run `wrepo sync` whenever you add or update parts in a repo.**

The index records for each part:
- Name, version, release, epoch, architecture
- Description
- Runtime and link dependencies
- Provides, conflicts, replaces
- Filename and SHA-256 checksum

A single part name can have multiple versions in the index. The resolver
collects all versions and picks the latest (or a user-specified version).

## Listing and Searching

```bash
wrepo list                   # all indexed parts
wrepo list gcc               # all available versions of gcc
wrepo search curl            # search by keyword (name + description)
```

Output marks the currently installed version with `[installed]`:

```
gcc 15.1.0-1 (x86_64) [installed]
gcc 14.2.0-3 (x86_64)
gcc 14.2.0-2 (x86_64)
```

## Removing Parts from the Index

```bash
wrepo remove gcc 14.2.0-2           # remove index entry only
wrepo remove gcc 14.2.0-2 --purge   # also delete the archive file
```

### Example index entry

```toml
[[parts]]
name = "curl"
version = "8.12.1"
release = 1
epoch = 0
description = "Command line tool and library for transferring data with URLs"
arch = "x86_64"
filename = "curl-8.12.1-1-x86_64.wright.tar.zst"
sha256 = "a1b2c3..."
install_size = 1234567
runtime_deps = ["glibc", "openssl", "zlib"]
link_deps = ["openssl", "zlib"]
```

## Upgrading

With an indexed repository, upgrades work by name:

```bash
wright upgrade gcc                       # upgrade to the latest available version
wright upgrade gcc --version 14.2.0      # switch to a specific version
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
wrepo source add team   --path /mnt/shared/wright-repo --priority 100
wrepo source add local  --path /var/lib/wright/myrepo   --priority 300
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
wrepo source add builds --path /mnt/nfs/wright-components
wright install @base
```

### Assembly build with repo

```bash
wbuild run -ic @qemu            # build, skip installed, install new packages
wrepo sync                      # update the index
```

## Tool Reference

| Tool | Role |
|------|------|
| `wright` | System administrator — install, remove, upgrade, query |
| `wbuild` | Package builder — build from plan.toml |
| `wrepo` | Repository manager — index, search, source management |
