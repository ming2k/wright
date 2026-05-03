# How to Inspect a Dependency Tree

## Print the Installed Dependency Tree

```bash
wright resolve gtk4 --tree
```

This shows the dependency tree from the installed part database.

## Show Reverse Dependents

```bash
wright resolve gtk4 --tree --rdeps
```

## Limit Depth

```bash
wright resolve gtk4 --tree --depth=2
```

## Filter by Dependency Type

```bash
wright resolve gtk4 --tree --deps=link
wright resolve gtk4 --tree --rdeps=link
```
