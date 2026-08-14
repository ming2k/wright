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

## Browse the Full Plan Graph in a Browser

```bash
wright graph --web
```

This starts a loopback-only web server and opens an interactive graph of
every discovered plan — installed or not — in your browser. Filter edges by
dependency domain (`build`, `link`, `runtime`) and nodes by state
(`installed`, `outdated`, `missing`, `external`), search by plan name, and
click a node for its details. The page is read-only; stop the server with
Ctrl-C. See [`wright graph`](../reference/cli-reference.md#wright-graph)
for the flag table.

