# Writing Assemblies

An **assembly** is a combinatory grouping of plans. It allows you to define sets of software that you want to manage together (e.g., a "base" system, a "desktop" environment, or a "development" toolchain).

Unlike plans, assemblies do not define how to build software; they simply point to existing plans.

## Directory Structure

Assemblies are stored in the directory defined by `assemblies_dir` in your `wright.toml` (default: `/var/lib/wright/assemblies/`).

Wright loads all `.toml` files in that directory.

```
/var/lib/wright/assemblies/
├── system.toml
└── workstation.toml
```

## Assembly Definition

An assembly is defined in a TOML file. The name of the file (minus the `.toml` extension) must match the `name` field inside the file.

### Basic Fields

| Field | Type | Description |
| :--- | :--- | :--- |
| `name` | string | **Required**. The unique name of the assembly. Must match the filename. |
| `description` | string | *Optional*. A brief description of what this assembly includes. |
| `plans` | list of strings | *Optional*. A list of plan names included in this assembly. |
| `includes` | list of strings | *Optional*. A list of other assembly names to include in this one. |

### Example

`base.toml`:
```toml
name = "base"
description = "Essential system parts"
plans = [
    "glibc",
    "bash",
    "coreutils",
    "grep",
    "sed",
    "util-linux"
]
```

`devel.toml`:
```toml
name = "devel"
description = "Core development toolchain"
plans = [
    "gcc",
    "make",
    "binutils",
    "pkgconf",
    "bison"
]
includes = ["base"]
```

## Using Assemblies

In the Wright CLI, assemblies are referenced using the `@` prefix.

### Building an Assembly

To build all plans defined in an assembly (and their dependencies):

```bash
wright build @base
```

### Applying an Assembly

To ensure your system matches the state described by an assembly:

```bash
wright apply @base
```

This will:
1. Resolve the requested plans and add missing or outdated dependencies.
2. Build what is needed in dependency order.
3. Install/upgrade parts to match the assembly targets.

### Combining Targets

You can mix individual plans and multiple assemblies in a single command:

```bash
wright apply @base @devel git vim
```

## Composition and Inheritance

Assemblies support composition through the `includes` field. This allows you to build layered system definitions.

`server.toml`:
```toml
name = "server"
plans = ["nginx", "openssl"]
includes = ["base"]
```

`web-dev.toml`:
```toml
name = "web-dev"
plans = ["nodejs", "python"]
includes = ["server", "devel"]
```

When you reference `@web-dev`, Wright will include all plans from `web-dev`, `server`, `devel`, and `base`.

## Best Practices

1. **Keep them Flat**: While assemblies can include other assemblies, try to keep the hierarchy shallow to make it easy to understand what software is being pulled in.
2. **Modular Files**: Group related assemblies into descriptive files (e.g., `networking.toml`, `graphics.toml`).
3. **Use Descriptions**: Descriptions are shown in `wright list --assemblies` (if supported) and help document your system configuration.
4. **No Dependency Logic**: Remember that assemblies are just lists. Actual ordering and dependency management are handled by the plans themselves.
