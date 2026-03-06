# Repositories

Wright resolves packages by name from configured **sources** — directories
containing `.wright.tar.zst` archives and an index file. This guide covers
creating, indexing, and managing local repositories.

## Concepts

| Term | What it is |
|------|-----------|
| **Source** | A configured directory (local) or URL (remote, future) where wright looks for packages |
| **Index** | A `wright.index.toml` file listing every package in a source with its metadata and checksum |
| **Resolver** | The component that finds a package archive by name — checks the index first, falls back to scanning archives |

## Quick Start

```bash
# 1. Create a repo directory
mkdir -p /var/lib/wright/myrepo

# 2. Build packages into it (or copy existing archives)
cp *.wright.tar.zst /var/lib/wright/myrepo/

# 3. Generate the index
wbuild index /var/lib/wright/myrepo

# 4. Register as a source
wright source add myrepo --path /var/lib/wright/myrepo

# 5. Verify
wright sync
wright search -a curl

# 6. Install by name
wright install curl
```

## Managing Sources

Sources are stored in `/etc/wright/repos.toml`. Use the `wright source`
commands to manage them without editing the file by hand.

### Add a source

```bash
wright source add myrepo --path /var/lib/wright/myrepo
wright source add myrepo --path /var/lib/wright/myrepo --priority 300
wright source add holdtree --type hold --path /var/lib/wright/plans
```

| Flag | Default | Description |
|------|---------|-------------|
| `--path <PATH>` | *(required)* | Local directory path |
| `--type <TYPE>` | `local` | `local` (binary packages) or `hold` (plan source tree) |
| `--priority <N>` | `100` | Higher number = preferred when the same package exists in multiple sources |

### List sources

```bash
wright source list
# myrepo          local    pri=300  /var/lib/wright/myrepo
# holdtree        hold     pri=100  /var/lib/wright/plans
```

### Remove a source

```bash
wright source remove myrepo
```

## Indexing

The index (`wright.index.toml`) is what makes name-based resolution fast.
Without it, the resolver must decompress and read `.PKGINFO` from every
archive in the directory.

```bash
wbuild index                              # index the default components_dir
wbuild index /var/lib/wright/myrepo       # index a specific directory
```

**Re-run `wbuild index` whenever you add, remove, or update packages in a repo.**

The index records for each package:
- Name, version, release, epoch, architecture
- Description
- Runtime and link dependencies
- Provides, conflicts, replaces
- Filename and SHA-256 checksum

### Example index entry

```toml
[[packages]]
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

## Syncing

```bash
wright sync
```

Reports the number of available packages from each indexed source. For local
repos this simply reads the existing index files — there is nothing to download.

## Searching Available Packages

```bash
wright search -a curl          # search available (indexed) packages
wright search curl             # search installed packages only
```

Available package results show an `[installed]` tag if the package is already
on the system.

## Multiple Repos

When the same package exists in multiple sources, the source with the
higher `priority` wins. This lets you layer a local build repo on top of
a shared team repo:

```bash
wright source add team   --path /mnt/shared/wright-repo --priority 100
wright source add local  --path /var/lib/wright/myrepo   --priority 300
```

Packages you build locally (priority 300) shadow the team repo (priority 100).

## Typical Workflows

### Personal build-and-install

```bash
wbuild run -i curl              # build and install in one step (no index needed)
```

### Shared team repo

```bash
# Builder machine
wbuild run @core
wbuild index /var/lib/wright/components

# Developer machines
wright source add builds --path /mnt/nfs/wright-components
wright sync
wright install @base
```

### Assembly build with repo

```bash
wbuild run -ic @qemu            # build, skip installed, install new packages
wbuild index                    # update the index with newly built packages
wright sync                     # refresh available package list
```
