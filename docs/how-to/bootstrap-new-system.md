# How to Bootstrap a New System

When deploying Wright onto a fresh LFS-based system, the core parts (glibc, gcc, binutils, linux, etc.) are already installed but unknown to Wright's database. Any part whose `plan.toml` lists them as dependencies will fail with an unresolved dependency error until they are registered.

## Seed the Database

Use `wright assume` to register the parts that already exist:

```sh
wright assume glibc 2.41
wright assume gcc 14.2.0
wright assume binutils 2.43
wright assume linux 6.12.0
wright assume bash 5.2
wright assume coreutils 9.5
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
bash 5.2 [external]
binutils 2.43 [external]
coreutils 9.5 [external]
gcc 14.2.0 [external]
glibc 2.41 [external]
man-db 2.12.1-1 (x86_64)
python 3.13.0-1 (x86_64)
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
