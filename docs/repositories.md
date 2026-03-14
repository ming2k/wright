# Repositories

Wright resolves parts by name from configured **sources** — directories
containing `.wright.tar.zst` archives and an index file. This guide covers
creating, indexing, and managing local repositories.

## Concepts

| Term | What it is |
|------|-----------|
| **Source** | A configured directory (local) or URL (remote, future) where wright looks for parts |
| **Index** | A `wright.index.toml` file listing every part in a source with its metadata and checksum |
| **Resolver** | The component that finds a part archive by name — checks the index first, falls back to scanning archives |

## Quick Start

```bash
# 1. Create a repo directory
mkdir -p /var/lib/wright/myrepo

# 2. Build packages into it (or copy existing archives)
cp *.wright.tar.zst /var/lib/wright/myrepo/

# 3. Generate the index
wright repo sync /var/lib/wright/myrepo

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
wright source list
# myrepo          local    pri=300  /var/lib/wright/myrepo
# holdtree        hold     pri=100  /var/lib/wright/plans
```

### Remove a source

```bash
wright source remove myrepo
```

## Repository Management

Use `wright repo` to manage local repositories directly.

### Indexing

The index (`wright.index.toml`) is what makes name-based resolution fast.
Without it, the resolver must decompress and read `.PARTINFO` from every
archive in the directory.

```bash
wright repo sync /var/lib/wright/myrepo   # generate/update index for a directory
wbuild index /var/lib/wright/myrepo       # alternative: also works from wbuild
```

**Re-run `wright repo sync` whenever you add or update parts in a repo.**

The index records for each part:
- Name, version, release, epoch, architecture
- Description
- Runtime and link dependencies
- Provides, conflicts, replaces
- Filename and SHA-256 checksum

A single part name can have multiple versions in the index. The resolver
collects all versions and picks the latest (or a user-specified version).

### Listing available parts

```bash
wright repo list                   # all indexed parts
wright repo list gcc               # all available versions of gcc
```

Output marks the currently installed version with `[installed]`:

```
gcc 15.1.0-1 (x86_64) [installed]
gcc 14.2.0-3 (x86_64)
gcc 14.2.0-2 (x86_64)
```

### Removing parts from the index

```bash
wright repo remove gcc 14.2.0-2           # remove index entry only
wright repo remove gcc 14.2.0-2 --purge   # also delete the archive file
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

## Syncing

```bash
wright sync
```

Reports the number of available parts from each indexed source. For local
repos this simply reads the existing index files — there is nothing to download.

## Searching Available Parts

```bash
wright search -a curl          # search available (indexed) parts
wright search curl             # search installed parts only
```

Available part results show an `[installed]` tag if the part is already
on the system.

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
wright source add team   --path /mnt/shared/wright-repo --priority 100
wright source add local  --path /var/lib/wright/myrepo   --priority 300
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
wright repo sync /var/lib/wright/components   # re-index
wright upgrade gcc              # upgrade to the newly built version
```

### Shared team repo

```bash
# Builder machine
wbuild run @core
wright repo sync /var/lib/wright/components

# Developer machines
wright source add builds --path /mnt/nfs/wright-components
wright sync
wright install @base
```

### Assembly build with repo

```bash
wbuild run -ic @qemu            # build, skip installed, install new packages
wright repo sync /var/lib/wright/components   # update the index
wright sync                     # refresh available part list
```
