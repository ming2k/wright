# How to Bootstrap a New System

When deploying Wright onto a fresh LFS-based system, the core parts (glibc, gcc, binutils, linux, etc.) are already installed but unknown to Wright's database. Any part whose `plan.toml` lists them as dependencies will fail with an unresolved dependency error until they are registered.

## Seed the Database

Use `wright assume` to register the parts that already exist.

### Single entry

```sh
wright assume glibc 2.41
```

### Bulk from a file

Create a file (e.g. `/etc/wright/bootstrap.txt`):

```
# Core toolchain
glibc 2.41
gcc 14.2.0
binutils 2.43
linux 6.12.0

# Base utilities
bash 5.2
coreutils 9.5
sed 4.9
grep 3.11
awk 5.3.0
tar 1.35
gzip 1.13
```

Then import it:

```sh
wright assume --file /etc/wright/bootstrap.txt
```

### Pipe multiple entries

```sh
cat <<EOF | wright assume
glibc 2.41
gcc 14.2.0
binutils 2.43
linux 6.12.0
bash 5.2
coreutils 9.5
EOF
```

## Install Parts Normally

After seeding, install parts normally:

```sh
wright install man-db-2.12.1-1.wright.tar.zst
wright install python-3.13.0-1.wright.tar.zst
```

## Verify Assumed Parts

Assumed parts appear with an `[external]` tag in `wright list`:

```
external     bash                     5.2
external     binutils                 2.43
external     coreutils                9.5
external     gcc                      14.2.0
external     glibc                    2.41
manual       man-db                   2.12.1-1-x86_64
manual       python                   3.13.0-1-x86_64
```

Or filter to assumed-only:

```sh
wright list --assumed
```

## Replace Assumed Parts

Once you have a Wright-built part ready to replace a stub, simply install it:

```sh
wright install glibc-2.41-1.wright.tar.zst
```

After that, `wright list` will show the fully managed part entry and `wright verify glibc` will check its file integrity as normal.

## Remove an Assumption

To remove an assumed record without installing a replacement:

```sh
wright unassume glibc
```

If you try to `wright remove` an assumed part, Wright will refuse and tell you to use `unassume` instead, because assumed parts are not managed by Wright and have no tracked files to delete.
