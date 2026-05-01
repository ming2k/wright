# How to Remove Packages and Clean Up Dependencies

## Remove a Part and Its Orphan Dependencies

Remove a part and its orphan dependencies (auto-installed deps no longer needed):

```bash
wright remove --cascade nginx
```

## List Orphan Parts

List orphan parts (auto-installed dependencies that nothing depends on anymore):

```bash
wright list --orphans
```

## Promote Install Origin

If you explicitly install a part that was previously pulled in as a dependency, its origin gets promoted to `manual` and won't be removed by `--cascade`:

```bash
# pcre was auto-installed as a dependency of nginx (origin: dependency)
wright install pcre-8.45-1-x86_64.wright.tar.zst
# pcre is now "manual" — cascade won't touch it
```
