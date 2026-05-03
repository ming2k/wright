# System Building Guide

This document is intended for users building and maintaining custom Linux distributions using Wright. It covers filesystem structure standards, packaging principles, and system maintenance strategies.

## 1. Filesystem Hierarchy Standard (FHS)

The target distribution for Wright follows a streamlined variant of the FHS, optimized for a musl + runit system.

### 1.1 Ideal Root Directory Structure

```
/
├── bin       → usr/bin  (usrmerge)
├── sbin      → usr/bin  (usrmerge; sbin is fully merged into bin)
├── lib       → usr/lib  (usrmerge)
├── lib64      → usr/lib  (or usr/lib64 on glibc multi-arch systems)
├── usr/
│  ├── bin/    # All executables
│  ├── lib/    # Shared libraries
│  ├── lib64/   # Multi-arch secondary library directory
│  ├── share/   # Architecture-independent data
│  ├── include/  # C/C++ header files
│  └── local/   # User-installed software (NOT managed by Wright)
├── etc/      # System configuration files
│  ├── wright/   # Wright system configuration
│  └── sv/     # runit service definitions
├── var/
│  ├── lib/    # Persistent program state data
│  │  └── wright/ # Wright database and cache
│  ├── log/    # Log files
│  ├── run/    # Runtime data (tmpfs)
│  └── tmp/    # Temporary files
├── home/      # User home directories
├── root/      # root user home directory
├── run/      # Runtime data (tmpfs)
├── tmp/      # Temporary files (tmpfs)
├── opt/      # Self-contained third-party software trees
├── boot/      # Kernel and bootloader files
├── dev/      # Device files (devtmpfs)
├── proc/      # Process information (procfs)
└── sys/      # Kernel interface (sysfs)
```

### 1.2 Key Design Decisions

For the ADR behind these decisions, see [ADR-0007: usrmerge and sbin Merged into bin](../adr/0007-usrmerge-and-sbin-merge.md).

**usrmerge**: `/bin`, `/sbin`, and `/lib` are all symlinks to their counterparts under `/usr/`. All parts must install files under `/usr/`.

**sbin merged into bin**: `/sbin` and `/usr/sbin` both resolve to `/usr/bin`. There is no separate `sbin` directory. Root-only tools live in `/usr/bin` like everything else; privilege is enforced by permissions, not path.

**lib64 handling**: On musl systems, `/lib64` is a symlink to `/usr/lib`. On glibc multi-arch systems, `/lib64` may point to `/usr/lib64`.

**No /usr/local pollution**: `/usr/local/` is reserved for software manually compiled and installed by the user. Parts managed by Wright must never install files there.

### 1.3 Part File Installation Conventions

| File Type | Installation Path | Description |
|-----------|-------------------|-------------|
| Executable | `/usr/bin/` | All executables |
| Shared Library | `/usr/lib/` | `.so` files and version symlinks |
| Static Library | `/usr/lib/` | `.a` files |
| Header File | `/usr/include/` | C/C++ development headers |
| pkg-config | `/usr/lib/pkgconfig/` | `.pc` files |
| cmake module | `/usr/lib/cmake/` | cmake find modules |
| Manual page | `/usr/share/man/` | man pages |
| Documentation | `/usr/share/doc/{partname}/` | README, LICENSE, etc. |
| Configuration | `/etc/` | System-level configuration |
| runit service | `/etc/sv/{service}/` | Service definition |
| Runtime data | `/var/lib/{partname}/` | Databases, state files |
| Logs | `/var/logs/{partname}/` | Log directory |

## 2. Packaging Principles

### 2.1 Core Philosophy

Splitting a part involves dividing the build products of a single source plan into multiple independent binary parts. Splitting increases complexity and should only be done when truly necessary.

### 2.2 Criteria for Splitting

The core question: **Do subsets of files produced from the same source tree have significantly different user groups or lifecycles?**

Consider:

1. **Runtime vs. Build-time**
2. **Size Difference**
3. **Dependency Propagation**
4. **Hardware/Scenario Specificity**

### 2.3 Typical Splitting Cases

#### GCC: Compiler vs. Runtime Libraries

GCC is the classic case for mandatory splitting:

```
gcc (source)
├── gcc       # C compiler
├── g++       # C++ compiler frontend
├── libstdc++    # C++ standard library runtime
├── libgcc      # GCC low-level runtime
├── libgomp     # OpenMP runtime
├── libatomic    # Atomic operations library
└── gcc-doc     # Documentation
```

**Why it must be split:**

- `libstdc++` and `libgcc` are runtime dependencies for almost all C++ programs. Without splitting, installing any C++ program would pull in the entire GCC compiler.
- Conversely, many parts need `libstdc++.so` but not `g++`.

#### linux-firmware: Split by Hardware

The `linux-firmware` repository contains firmware binaries for all hardware, totaling over 800MB. Most users only need firmware for their own hardware.

**Why it must be split:**

- 800MB of firmware is mostly wasted on a personal system.
- There are almost no dependencies between firmware files.

#### More Quick Reference Cases

| Project | Splitting Method | Reason |
|---------|-----------------|--------|
| **dbus** | `libdbus` + `dbus-daemon` | Many programs link against libdbus but don't need the daemon |
| **Python** | `python` + `python-doc` | Documentation ~50MB, not needed at runtime |
| **Mesa** | `mesa-dri` + `mesa-vulkan-intel` + ... | Different GPU drivers are unrelated |
| **util-linux** | `libblkid` + `libuuid` + `libmount` + `util-linux` | Libraries are widely linked |
| **glib** | No split | Library and tools are tightly coupled |
| **zlib** | No split | Small library, splitting is meaningless |
| **curl** | No split | Both libcurl and curl CLI are small |

### 2.4 When NOT to Split

| Scenario | Reason |
|----------|--------|
| `-dev` parts (for personal/small team use) | Disk space is far less important than maintenance complexity |
| Small libraries | Space saved doesn't justify increased dependency complexity |
| Tightly coupled components | Components almost always used together |
| Parts with a single use case | No diverse user base |

### 2.5 Recommendations for -dev Splitting

For the ADR behind this decision, see [ADR-0008: No -dev Splitting](../adr/0008-no-dev-splitting.md).

Traditional distributions split headers, static libraries, and pkg-config files into `-dev` sub-parts. This is reasonable in large-scale public repositories or embedded environments.

For Wright's target users (personal or small team maintained custom distributions), **NOT splitting -dev is a better default choice**:

1. **Maintenance Cost**: Each `-dev` split means extra dependency declarations, version tracking, and testing.
2. **Build-friendly**: Not splitting `-dev` means installing a library allows you to immediately compile software that depends on it.
3. **Debug-friendly**: Header files are often useful during troubleshooting.
4. **Negligible Disk Overhead**: Headers and `.pc` files typically take up only a few hundred KB.

**Exception**: If development files are exceptionally large (e.g., Qt, LLVM headers > 50MB), consider splitting.

### 2.6 Multi-Part Practice

Use `[[output]]` array-of-tables in `plan.toml` to define sub-parts. See [Writing Plans](../reference/writing-plans.md) for details.

### 2.7 Decision Summary Table

| Question | Answer "Yes" → Split | Answer "No" → Don't Split |
|----------|----------------------|---------------------------|
| Does the subset have an independent user group? | Split | Don't Split |
| Does the subset exceed 30% of the main part's size? | Split | Don't Split |
| Does splitting reduce at least 2 unnecessary dependency chains? | Split | Don't Split |
| Is the subset truly unnecessary at runtime? | Split | Don't Split |

Consider splitting when at least two "Yes" answers are met.

## 3. Dependency Management Strategy

### 3.1 Strict Protection of Runtime Dependencies

By default, Wright **prohibits the removal of software that other installed parts depend on**.

Removal protection follows recorded installed/runtime dependencies.

### 3.2 Dependency Declaration Principles

- **runtime**: Parts that must exist at runtime.
- **build**: Required only during build time; not recorded in binary parts.
- **link**: ABI-sensitive build edge used for rebuild propagation. It may overlap with `runtime`.
- **optional**: Enhances functionality but is not mandatory.

```toml
[dependencies]
runtime = ["bash"]
build = ["gcc", "make", "cmake"]
link = ["zlib", "openssl >= 3.0"]
optional = ["nghttp2"]
```

### 3.3 Avoiding Circular Dependencies

Circular dependencies are detected and rejected by Wright's dependency resolver. If you encounter them:

1. Determine if it's a true runtime circular dependency.
2. Change one direction to `optional` or handle it in `build` dependencies.
3. If necessary, merge them into a single part.

For cycles that cannot be broken by classification, see [ADR-0006: MVP Two-Pass Build](../adr/0006-mvp-two-pass-build.md).

## 4. Build Conventions

### 4.1 Common Compilation Flags

- `-O2`: Balanced optimization and compilation speed.
- `-pipe`: Uses pipes instead of temporary files.
- `-march=x86-64`: Baseline x86_64 compatibility.

### 4.2 musl Compatibility Notes

- No `execinfo.h` (backtrace support).
- `GLOB_TILDE` / `GLOB_BRACE` unavailable.
- Limited locale support.
- Some software assumes glibc-specific headers.

### 4.3 runit Service Packaging

Parts providing daemons must include a runit service directory:

```
/etc/sv/{service}/run     # Required, service start script
/etc/sv/{service}/finish    # Optional, cleanup script
/etc/sv/{service}/logs/run   # Recommended, logging script
```

Services are **NOT enabled by default**. Users enable services via symlinks:

```sh
ln -s /etc/sv/nginx /var/service/
```

### 4.4 Configuration File Protection

Declare configuration files in the `backup` field of `[output]`:

```toml
[output]
backup = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

## 5. Repository Tiers and Part Categories

### 5.1 Four-Tier Repository Structure

| Tier | Name | Content | Update Policy |
|------|------|---------|---------------|
| **core** | Core System | Toolchain, libc, kernel, init, essential utilities | Extremely conservative, security fixes only |
| **base** | Base System | Networking tools, filesystem tools, common libraries | Stable versions, promoted after testing |
| **extra** | Extra Parts | Servers, language runtimes, development tools | Track stable dependency |
| **community**| Community | User-contributed parts | No stability guarantees |

### 5.2 Software NOT Included in Native Repositories

- Desktop applications with complex dependency trees → Use Flatpak.
- Dependency software that is no longer maintained.
- Software that only supports glibc and cannot be reasonably patched.
- Closed-source software.
