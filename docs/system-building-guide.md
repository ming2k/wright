# System Building Guide

This document is intended for users building and maintaining custom Linux distributions using Wright. It covers filesystem structure standards, packaging principles, and system maintenance strategies.

---

## 1. Filesystem Hierarchy Standard (FHS)

The target distribution for Wright follows a streamlined variant of the FHS (Filesystem Hierarchy Standard), optimized for a musl + runit system.

### 1.1 Ideal Root Directory Structure

```
/
├── bin/            → symlink to /usr/bin (usrmerge)
├── sbin/           → symlink to /usr/sbin (usrmerge)
├── lib/            → symlink to /usr/lib (usrmerge)
├── usr/
│   ├── bin/        # All user and system executables
│   ├── sbin/       # System management tools (optional, can be merged into bin/)
│   ├── lib/        # Shared libraries (.so) and internal libraries
│   ├── libexec/    # Helper executables for internal program use
│   ├── include/    # C/C++ header files
│   ├── share/      # Architecture-independent data
│   │   ├── man/    # Manual pages
│   │   ├── doc/    # Documentation
│   │   ├── info/   # Info pages
│   │   └── locale/ # Localization files
│   └── local/      # User-installed software (NOT managed by Wright)
├── etc/            # System configuration files
│   ├── wright/     # Wright package manager configuration
│   ├── sv/         # runit service definitions
│   └── ...
├── var/
│   ├── lib/        # Persistent program state data
│   │   └── wright/ # Wright database and cache
│   ├── log/        # Log files
│   ├── run/        # Runtime data (tmpfs)
│   ├── service/    → symlinks to enabled services in /etc/sv
│   ├── hold/       # Hold tree (collection of plan files)
│   └── tmp/        # Temporary files (cleared on reboot)
├── dev/            # Device files (devtmpfs)
├── proc/           # Process information (procfs)
├── sys/            # Kernel interface (sysfs)
├── tmp/            # Temporary files (tmpfs, world-writable)
├── run/            → symlink to /var/run (or independent tmpfs)
├── boot/           # Kernel and bootloader files
├── home/           # User home directories
└── root/           # root user home directory
```

### 1.2 Key Design Decisions

**usrmerge**: `/bin`, `/sbin`, and `/lib` are all symlinks to their corresponding directories under `/usr/`. All packages should install files under `/usr/`. This simplifies the system structure and avoids historical fragmentation between `/bin` and `/usr/bin`.

**No /lib64**: musl libc does not distinguish between `lib` and `lib64`. All libraries are installed in `/usr/lib/`.

**No /opt**: The `/opt` directory is not used. All native packages are installed in standard FHS locations. For isolated third-party software, use Flatpak.

**No /usr/local pollution**: `/usr/local/` is reserved for software manually compiled and installed by the user. Packages managed by Wright should never install files into this directory.

### 1.3 Package File Installation Conventions

| File Type | Installation Path | Description |
|-----------|-------------------|-------------|
| Executable | `/usr/bin/` | Unified location for user and system commands |
| Management Tool | `/usr/sbin/` | Root-only management tools (optional) |
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
| Logs | `/var/log/{pkgname}/` | Log directory |

---

## 2. Packaging Principles

### 2.1 Core Philosophy

Splitting a package (split package) involves dividing the build products of a single upstream source package into multiple independent binary packages. Splitting increases complexity and should only be done when truly necessary.

### 2.2 Criteria for Splitting

The core question for splitting is: **Do subsets of files produced from the same source tree have significantly different user groups or lifecycles?**

Consider the following factors:

1.  **Runtime vs. Build-time**: Many programs only need a library's `.so` file, not the compiler itself.
2.  **Size Difference**: A subset is huge and unnecessary for most users.
3.  **Dependency Propagation**: Not splitting causes many packages to pull in heavy, unnecessary components.
4.  **Hardware/Scenario Specificity**: Firmware, drivers, etc., are only relevant to specific hardware.

### 2.3 Typical Splitting Cases

#### GCC: Compiler vs. Runtime Libraries

GCC is the classic case for mandatory splitting. A single build produces the compiler and multiple runtime libraries:

```
gcc (source)
├── gcc              # C compiler, cc1, collect2, etc. (~100MB+)
├── g++              # C++ compiler frontend
├── libstdc++        # C++ standard library runtime (~5MB)
├── libgcc           # GCC low-level runtime (~200KB)
├── libgomp          # OpenMP runtime
├── libatomic        # Atomic operations library
└── gcc-doc          # Documentation (info/man, large)
```

**Why it must be split:**

- `libstdc++` and `libgcc` are runtime dependencies for almost all C++ programs. Without splitting, installing any C++ program would pull in the entire GCC compiler (100MB+), which is unacceptable.
- Conversely, many packages need `libstdc++.so` but not `g++`.
- The same applies to small runtime libraries like `libgomp` and `libatomic`—they are needed for execution, not for compilation.

```toml
# gcc/plan.toml splitting example
[split.libstdc++]
description = "GNU C++ standard library runtime"
files = ["/usr/lib/libstdc++.so*"]
dependencies = ["libgcc"]

[split.libgcc]
description = "GCC low-level runtime library"
files = ["/usr/lib/libgcc_s.so*"]
dependencies = []

[split.libgomp]
description = "GNU OpenMP runtime"
files = ["/usr/lib/libgomp.so*"]
dependencies = ["libgcc"]

[split.libatomic]
description = "GNU atomic operations library"
files = ["/usr/lib/libatomic.so*"]
dependencies = ["libgcc"]

[split.doc]
description = "GCC documentation"
files = ["/usr/share/doc/gcc/*", "/usr/share/man/man7/*", "/usr/share/info/gcc*"]
```

#### linux-firmware: Split by Hardware

The upstream `linux-firmware` repository contains firmware binaries for all hardware, totaling over 800MB. Most users only need the firmware corresponding to their own hardware.

```
linux-firmware (source, ~800MB+)
├── linux-firmware-amdgpu      # AMD GPU firmware (~150MB)
├── linux-firmware-nvidia       # NVIDIA Nouveau firmware
├── linux-firmware-intel        # Various Intel firmware (WiFi, GPU, etc.)
├── linux-firmware-iwlwifi      # Intel wireless firmware
├── linux-firmware-realtek      # Realtek network/WiFi firmware
├── linux-firmware-ath          # Atheros/Qualcomm WiFi firmware
├── linux-firmware-broadcom     # Broadcom firmware
└── ...
```

**Why it must be split:**

- 800MB of firmware is mostly wasted on a personal system—a machine usually only needs 2-3 subpackages.
- There are almost no dependencies between firmware files, making them ideal for splitting.
- By splitting by hardware vendor/type, users only install the firmware their hardware requires.

```toml
# linux-firmware/plan.toml splitting example
[split.amdgpu]
description = "AMD GPU firmware"
files = ["/usr/lib/firmware/amdgpu/*"]
dependencies = []

[split.iwlwifi]
description = "Intel wireless firmware"
files = ["/usr/lib/firmware/iwlwifi-*"]
dependencies = []

[split.realtek]
description = "Realtek firmware"
files = ["/usr/lib/firmware/rtl_nic/*", "/usr/lib/firmware/rtlwifi/*", "/usr/lib/firmware/rtw88/*", "/usr/lib/firmware/rtw89/*"]
dependencies = []
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
| `-dev` packages (for personal/small team use) | Disk space is far less important than maintenance complexity (see 2.5) |
| Small libraries | Space saved doesn't justify increased dependency complexity |
| Tightly coupled components | Components almost always used together should not be split |
| Packages with a single use case | No diverse user base, no beneficiaries of splitting |

### 2.5 Recommendations for -dev Splitting

Traditional distributions (Debian, Alpine) split header files (`.h`), static libraries (`.a`), and pkg-config (`.pc`) files into `-dev` subpackages. This is reasonable in:

- **Large-scale public repositories**: Thousands of users, most are end-users who don't need development files.
- **Embedded/Container environments**: Extremely limited disk space.

However, for Wright's target users (personal or small team maintained custom distributions), **NOT splitting -dev is a better default choice**:

1.  **Maintenance Cost**: Each `-dev` split means extra dependency declarations, version tracking, and testing.
2.  **Build-friendly**: Not splitting `-dev` means installing a library allows you to immediately compile software that depends on it, without needing extra steps.
3.  **Debug-friendly**: Header files are often useful during troubleshooting.
4.  **Negligible Disk Overhead**: Headers and `.pc` files typically take up only a few hundred KB.

**Exception**: If a package's development files are exceptionally large (e.g., Qt, LLVM headers over 50MB), consider splitting.

### 2.6 Splitting Practice

Use the `[split]` section in `plan.toml` to define subpackages:

```toml
# Example: Splitting large documentation only
[split.doc]
description = "GCC documentation"
files = ["/usr/share/doc/gcc/*", "/usr/share/man/man7/*", "/usr/share/info/gcc*"]

# Example: Library and daemon split
[split.libs]
description = "D-Bus shared libraries"
files = ["/usr/lib/libdbus-1.so*"]
dependencies = []
```

### 2.7 Decision Summary Table

| Question | Answer "Yes" → Split | Answer "No" → Don't Split |
|----------|----------------------|---------------------------|
| Does the subset of files have an independent user group? | Split | Don't Split |
| Does the subset exceed 30% of the main package's size? | Split | Don't Split |
| Does splitting reduce at least 2 unnecessary dependency chains? | Split | Don't Split |
| Is the subset truly unnecessary at runtime? | Split | Don't Split |

Consider splitting when at least two "Yes" answers are met.

---

## 3. Dependency Management Strategy

### 3.1 Strict Protection of Runtime Dependencies

By default, Wright **prohibits the removal of software that other installed packages depend on**.

Specifically, for **link dependencies**, Wright enforces even stricter protection:
- If you attempt to remove a library that other packages depend on via `link` (e.g., `openssl`), the removal will be **blocked** with a **CRITICAL** error.
- This is because the absence of a `link` dependency causes dependent programs to crash immediately due to missing shared libraries.

```
$ wright remove openssl
error: CRITICAL: Cannot remove 'openssl' because it is a LINK dependency of: curl, nginx, git. Removing it will cause these packages to CRASH. Use --force to override.
```

### 3.2 Dependency Declaration Principles

- **runtime**: Packages that must exist at runtime. E.g., script interpreters, non-linked libraries. Declare only direct dependencies, not transitive ones.
- **build**: Required only during build time; not recorded in binary packages. E.g., compilers, build tools.
- **link**: Required at build time (headers/libs) and runtime (shared libraries). **Key Point**: When a `link` dependency is updated, Wright automatically triggers a rebuild of this package to ensure binary compatibility (ABI match).
- **optional**: Enhances functionality but is not mandatory; provided as informational only.

```toml
[dependencies]
runtime = ["bash"]
build = ["gcc", "make", "cmake"]
link = ["zlib", "openssl >= 3.0"]
optional = [
    { name = "nghttp2", description = "HTTP/2 support" },
]
```

### 3.3 Avoiding Circular Dependencies

Circular dependencies (A → B → A) are detected and rejected by Wright's dependency resolver. If you encounter circular dependencies upstream:

1.  Determine if it's a true runtime circular dependency (usually it's not).
2.  Change one direction to `optional` or handle it in `build` dependencies.
3.  If necessary, consider merging them into a single package.

---

## 4. Build Conventions

### 4.1 Common Compilation Flags

Recommended default compilation flags (configured in `wright.toml`):

```toml
[build]
cflags = "-O2 -pipe -march=x86-64"
cxxflags = "${cflags}"
```

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

Packages providing daemons must include a runit service directory:

```
/etc/sv/{service}/run          # Required, service start script
/etc/sv/{service}/finish       # Optional, cleanup script
/etc/sv/{service}/log/run      # Recommended, logging script
```

Services are **NOT enabled by default**. Users enable services via symlinks:

```sh
ln -s /etc/sv/nginx /var/service/
```

### 4.4 Configuration File Protection

Declare configuration files to be protected in the `[backup]` section of `plan.toml`. These files are preserved during uninstallation and not overwritten during upgrades:

```toml
[backup]
files = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

---

## 5. Repository Tiers and Package Categories

### 5.1 Four-Tier Repository Structure

| Tier | Name | Content | Update Policy |
|------|------|---------|---------------|
| **core** | Core System | Toolchain, libc, kernel, init, essential utilities | Extremely conservative, security fixes only |
| **base** | Base System | Networking tools, filesystem tools, common libraries, Wright itself | Stable versions, promoted after testing against core |
| **extra** | Extra Packages | Servers, language runtimes, development tools | Track stable upstream |
| **community**| Community | User-contributed packages | No stability guarantees |

### 5.2 Software NOT Included in Native Repositories

- Desktop applications with complex dependency trees → Use Flatpak.
- Upstream software that is no longer maintained.
- Software that only supports glibc and cannot be reasonably patched.
- Closed-source software.