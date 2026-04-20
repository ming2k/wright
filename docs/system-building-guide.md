# System Building Guide

This document is intended for users building and maintaining custom Linux distributions using Wright. It covers filesystem structure standards, packaging principles, and system maintenance strategies.

---

## 1. Filesystem Hierarchy Standard (FHS)

The target distribution for Wright follows a streamlined variant of the FHS (Filesystem Hierarchy Standard), optimized for a musl + runit system.

### 1.1 Ideal Root Directory Structure

```
/
├── bin       → usr/bin  (usrmerge)
├── sbin      → usr/bin  (usrmerge; sbin is fully merged into bin)
├── lib       → usr/lib  (usrmerge)
├── lib64      → usr/lib  (or usr/lib64 on glibc multi-arch systems)
├── usr/
│  ├── bin/    # All executables — user commands and root tools unified
│  ├── lib/    # Shared libraries (.so) and internal libraries
│  ├── lib64/   # Multi-arch secondary library directory (if needed)
│  ├── share/   # Architecture-independent data
│  │  ├── man/  # Manual pages
│  │  ├── doc/  # Documentation
│  │  ├── info/  # Info pages
│  │  └── locale/ # Localization files
│  ├── include/  # C/C++ header files
│  └── local/   # User-installed software (NOT managed by Wright)
├── etc/      # System configuration files
│  ├── wright/   # Wright system configuration
│  ├── sv/     # runit service definitions
│  └── ...
├── var/
│  ├── lib/    # Persistent program state data
│  │  └── wright/ # Wright database and cache
│  ├── log/    # Log files
│  ├── run/    # Runtime data (tmpfs)
│  ├── service/  → symlinks to enabled services in /etc/sv
│  ├── hold/    # Hold tree (collection of plan files)
│  └── tmp/    # Temporary files (cleared on reboot)
├── home/      # User home directories
├── root/      # root user home directory
├── run/      # Runtime data (tmpfs, or symlink to /var/run)
├── tmp/      # Temporary files (tmpfs, world-writable)
├── opt/      # Self-contained third-party software trees
├── boot/      # Kernel and bootloader files
├── dev/      # Device files (devtmpfs)
├── proc/      # Process information (procfs)
└── sys/      # Kernel interface (sysfs)
```

### 1.2 Key Design Decisions

**usrmerge**: `/bin`, `/sbin`, and `/lib` are all symlinks to their counterparts under `/usr/`. All parts must install files under `/usr/`. This eliminates the historical split between `/bin` and `/usr/bin`.

**sbin merged into bin**: `/sbin` and `/usr/sbin` both resolve to `/usr/bin`. There is no separate `sbin` directory. Root-only tools (e.g. `mount`, `ip`) live in `/usr/bin` like everything else; privilege is enforced by permissions, not path.

**lib64 handling**: On musl systems, `/lib64` is a symlink to `/usr/lib` — musl does not distinguish `lib` from `lib64`. On glibc multi-arch systems, `/lib64` may point to `/usr/lib64` instead, and `/usr/lib64/` holds the secondary architecture's libraries. All parts should target `/usr/lib/` unless explicitly building for a secondary architecture.

**No /usr/local pollution**: `/usr/local/` is reserved for software manually compiled and installed by the user. Parts managed by Wright must never install files there.

**opt for third-party trees**: `/opt` is available for self-contained third-party software (e.g. proprietary bundles, Flatpak runtimes) that cannot conform to standard FHS paths. Native Wright parts do not install into `/opt`.

### 1.3 Package File Installation Conventions

| File Type | Installation Path | Description |
|-----------|-------------------|-------------|
| Executable | `/usr/bin/` | All executables — user commands and root tools unified |
| Shared Library | `/usr/lib/` | `.so` files and version symlinks |
| Static Library | `/usr/lib/` | `.a` files |
| Header File | `/usr/include/` | C/C++ development headers |
| pkg-config | `/usr/lib/pkgconfig/` | `.pc` files |
| cmake module | `/usr/lib/cmake/` | cmake find modules |
| Manual page | `/usr/share/man/` | man pages |
| Documentation | `/usr/share/doc/{pkgname}/` | README, LICENSE, etc. |
| Configuration | `/etc/` | System-level configuration |
| runit service | `/etc/sv/{service}/` | Service definition |
| Runtime data | `/var/lib/{pkgname}/` | Databases, state files, etc. |
| Logs | `/var/logs/{pkgname}/` | Log directory |

---

## 2. Packaging Principles

### 2.1 Core Philosophy

Splitting a part (split part) involves dividing the build products of a single upstream source plan into multiple independent binary parts. Splitting increases complexity and should only be done when truly necessary.

### 2.2 Criteria for Splitting

The core question for splitting is: **Do subsets of files produced from the same source tree have significantly different user groups or lifecycles?**

Consider the following factors:

1. **Runtime vs. Build-time**: Many programs only need a library's `.so` file, not the compiler itself.
2. **Size Difference**: A subset is huge and unnecessary for most users.
3. **Dependency Propagation**: Not splitting causes many parts to pull in heavy, unnecessary components.
4. **Hardware/Scenario Specificity**: Firmware, drivers, etc., are only relevant to specific hardware.

### 2.3 Typical Splitting Cases

#### GCC: Compiler vs. Runtime Libraries

GCC is the classic case for mandatory splitting. A single build produces the compiler and multiple runtime libraries:

```
gcc (source)
├── gcc       # C compiler, cc1, collect2, etc. (~100MB+)
├── g++       # C++ compiler frontend
├── libstdc++    # C++ standard library runtime (~5MB)
├── libgcc      # GCC low-level runtime (~200KB)
├── libgomp     # OpenMP runtime
├── libatomic    # Atomic operations library
└── gcc-doc     # Documentation (info/man, large)
```

**Why it must be split:**

- `libstdc++` and `libgcc` are runtime dependencies for almost all C++ programs. Without splitting, installing any C++ program would pull in the entire GCC compiler (100MB+), which is unacceptable.
- Conversely, many parts need `libstdc++.so` but not `g++`.
- The same applies to small runtime libraries like `libgomp` and `libatomic`—they are needed for execution, not for compilation.

```toml
# gcc/plan.toml multi-part example
[output."libstdc++"]
description = "GNU C++ standard library runtime"
hooks.post_install = "ldconfig"

[output."libstdc++".dependencies]
runtime = ["libgcc"]

[output.libgcc]
description = "GCC low-level runtime library"

[output.libgomp]
description = "GNU OpenMP runtime"

[output.libgomp.dependencies]
runtime = ["libgcc"]

[output.libatomic]
description = "GNU atomic operations library"

[output.libatomic.dependencies]
runtime = ["libgcc"]

[output.doc]
description = "GCC documentation"
script = """
"""
```

#### linux-firmware: Split by Hardware

The upstream `linux-firmware` repository contains firmware binaries for all hardware, totaling over 800MB. Most users only need the firmware corresponding to their own hardware.

```
linux-firmware (source, ~800MB+)
├── linux-firmware-amdgpu   # AMD GPU firmware (~150MB)
├── linux-firmware-nvidia    # NVIDIA Nouveau firmware
├── linux-firmware-intel    # Various Intel firmware (WiFi, GPU, etc.)
├── linux-firmware-iwlwifi   # Intel wireless firmware
├── linux-firmware-realtek   # Realtek network/WiFi firmware
├── linux-firmware-ath     # Atheros/Qualcomm WiFi firmware
├── linux-firmware-broadcom   # Broadcom firmware
└── ...
```

**Why it must be split:**

- 800MB of firmware is mostly wasted on a personal system—a machine usually only needs 2-3 sub-parts.
- There are almost no dependencies between firmware files, making them ideal for splitting.
- By splitting by hardware vendor/type, users only install the firmware their hardware requires.

```toml
# linux-firmware/plan.toml multi-part example
[output.amdgpu]
description = "AMD GPU firmware"

[output.iwlwifi]
description = "Intel wireless firmware"

[output.realtek]
description = "Realtek firmware"
script = """
"""
```

#### More Quick Reference Cases

| Upstream Project | Splitting Method | Reason |
|------------------|------------------|--------|
| **dbus** | `libdbus` + `dbus-daemon` | Many programs link against libdbus but don't need the daemon itself |
| **Python** | `python` + `python-doc` | Documentation ~50MB, not needed at runtime |
| **Mesa** | `mesa-dri` + `mesa-vulkan-intel` + `mesa-vulkan-radeon` + ... | Different GPU drivers are unrelated |
| **systemd** (if applicable) | `libsystemd` + `libudev` + `systemd` | Many programs only link against libudev, not the init system |
| **util-linux** | `libblkid` + `libuuid` + `libmount` + `util-linux` | Libraries are widely linked, but the toolset isn't needed by everyone |
| **glib** | No split | Library and tools are tightly coupled, reasonable size, almost always used together |
| **zlib** | No split | Small library, splitting is meaningless |
| **curl** | No split | Both `libcurl` and `curl` CLI are small and often needed together |

### 2.4 When NOT to Split

| Scenario | Reason |
|----------|--------|
| `-dev` parts (for personal/small team use) | Disk space is far less important than maintenance complexity (see 2.5) |
| Small libraries | Space saved doesn't justify increased dependency complexity |
| Tightly coupled components | Components almost always used together should not be split |
| Parts with a single use case | No diverse user base, no beneficiaries of splitting |

### 2.5 Recommendations for -dev Splitting

Traditional distributions (Debian, Alpine) split header files (`.h`), static libraries (`.a`), and pkg-config (`.pc`) files into `-dev` subpackages. This is reasonable in:

- **Large-scale public repositories**: Thousands of users, most are end-users who don't need development files.
- **Embedded/Container environments**: Extremely limited disk space.

However, for Wright's target users (personal or small team maintained custom distributions), **NOT splitting -dev is a better default choice**:

1. **Maintenance Cost**: Each `-dev` split means extra dependency declarations, version tracking, and testing.
2. **Build-friendly**: Not splitting `-dev` means installing a library allows you to immediately compile software that depends on it, without needing extra steps.
3. **Debug-friendly**: Header files are often useful during troubleshooting.
4. **Negligible Disk Overhead**: Headers and `.pc` files typically take up only a few hundred KB.

**Exception**: If a part's development files are exceptionally large (e.g., Qt, LLVM headers over 50MB), consider splitting.

### 2.6 Multi-Part Practice

Use `[lifecycle.fabricate.<name>]` sub-tables in `plan.toml` to define sub-parts:

```toml
# Example: Splitting large documentation only
[lifecycle.fabricate.doc]
description = "GCC documentation"
script = """
"""

# Example: Library and daemon split
[lifecycle.fabricate.libs]
description = "D-Bus shared libraries"
hooks.post_install = "ldconfig"
```

### 2.7 Decision Summary Table

| Question | Answer "Yes" → Split | Answer "No" → Don't Split |
|----------|----------------------|---------------------------|
| Does the subset of files have an independent user group? | Split | Don't Split |
| Does the subset exceed 30% of the main part's size? | Split | Don't Split |
| Does splitting reduce at least 2 unnecessary dependency chains? | Split | Don't Split |
| Is the subset truly unnecessary at runtime? | Split | Don't Split |

Consider splitting when at least two "Yes" answers are met.

---

## 3. Dependency Management Strategy

### 3.1 Strict Protection of Runtime Dependencies

By default, Wright **prohibits the removal of software that other installed parts depend on**.

Removal protection follows recorded installed/runtime dependencies. If removing a part would break installed parts, Wright blocks the removal unless forced.

```
$ wright remove openssl
error: CRITICAL: Cannot remove 'openssl' because it is a LINK dependency of: curl, nginx, git. Removing it will cause these parts to CRASH. Use --force to override.
```

### 3.2 Dependency Declaration Principles

- **runtime**: Parts that must exist at runtime. E.g., script interpreters, shared libraries, helper binaries. Declare only direct dependencies, not transitive ones.
- **build**: Required only during build time; not recorded in binary parts. E.g., compilers, build tools.
- **link**: ABI-sensitive build edge used for rebuild propagation. It may overlap with `runtime`, and often should for shared libraries. **Key Point**: When a `link` dependency is updated, Wright automatically triggers a rebuild of this part to ensure binary compatibility (ABI match). `link` alone does not make the dependency a recorded runtime requirement.
- **optional**: Enhances functionality but is not mandatory; provided as informational only.

```toml
[dependencies]
runtime = ["bash"]
build = ["gcc", "make", "cmake"]
link = ["zlib", "openssl >= 3.0"]
optional = ["nghttp2"]
```

### 3.3 Avoiding Circular Dependencies

Circular dependencies (A → B → A) are detected and rejected by Wright's dependency resolver. If you encounter circular dependencies upstream:

1. Determine if it's a true runtime circular dependency (usually it's not).
2. Change one direction to `optional` or handle it in `build` dependencies.
3. If necessary, consider merging them into a single part.

---

## 4. Build Conventions

### 4.1 Common Compilation Flags

Prefer setting compilation flags per plan or per stage, instead of relying on a
global config knob:

- `-O2`: Balanced optimization and compilation speed.
- `-pipe`: Uses pipes instead of temporary files, speeding up compilation.
- `-march=x86-64`: Baseline x86_64 compatibility.

### 4.2 musl Compatibility Notes

Pay attention to musl-specific issues during packaging:

- No `execinfo.h` (backtrace support); may require `libexecinfo` or patches.
- `GLOB_TILDE` / `GLOB_BRACE` unavailable.
- Limited locale support.
- Some software assumes glibc-specific header files exist.

When encountering compatibility issues, prioritize submitting patches upstream; if unresolved, distribute via Flatpak (which uses its own glibc runtime).

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

Declare configuration files to be protected in the `backup` field of `[output]` in `plan.toml`. These files are preserved during uninstallation and not overwritten during upgrades:

```toml
[output]
backup = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

---

## 5. Repository Tiers and Part Categories

### 5.1 Four-Tier Repository Structure

| Tier | Name | Content | Update Policy |
|------|------|---------|---------------|
| **core** | Core System | Toolchain, libc, kernel, init, essential utilities | Extremely conservative, security fixes only |
| **base** | Base System | Networking tools, filesystem tools, common libraries, Wright itself | Stable versions, promoted after testing against core |
| **extra** | Extra Parts | Servers, language runtimes, development tools | Track stable upstream |
| **community**| Community | User-contributed parts | No stability guarantees |

### 5.2 Software NOT Included in Native Repositories

- Desktop applications with complex dependency trees → Use Flatpak.
- Upstream software that is no longer maintained.
- Software that only supports glibc and cannot be reasonably patched.
- Closed-source software.
