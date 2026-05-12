# Filesystem Hierarchy Standard

Wright's target distribution follows a streamlined FHS variant, optimized for a musl + runit system.

## Root Directory Structure

```
/
├── bin       → usr/bin  (usrmerge)
├── sbin      → usr/bin  (usrmerge; sbin is fully merged into bin)
├── lib       → usr/lib  (usrmerge)
├── lib64     → usr/lib  (or usr/lib64 on glibc multi-arch systems)
├── usr/
│  ├── bin/        # All executables
│  ├── lib/        # Shared libraries
│  ├── lib64/      # Multi-arch secondary library directory
│  ├── share/      # Architecture-independent data
│  ├── include/    # C/C++ header files
│  └── local/      # User-installed software (NOT managed by Wright)
├── etc/
│  ├── wright/     # Wright system configuration
│  └── sv/         # runit service definitions
├── var/
│  ├── lib/        # Persistent program state data
│  │  └── wright/  # Wright database and cache
│  ├── log/        # Log files
│  ├── run/        # Runtime data (tmpfs)
│  └── tmp/        # Temporary files
├── home/          # User home directories
├── root/          # root user home directory
├── run/           # Runtime data (tmpfs)
├── tmp/           # Temporary files (tmpfs)
├── opt/           # Self-contained third-party software trees
├── boot/          # Kernel and bootloader files
├── dev/           # Device files (devtmpfs)
├── proc/          # Process information (procfs)
└── sys/           # Kernel interface (sysfs)
```

## Key Design Decisions

For the ADRs behind these decisions, see:
- [ADR-0007: usrmerge and sbin Merged into bin](../adr/0007-usrmerge-and-sbin-merge.md)
- [ADR-0008: No -dev Splitting](../adr/0008-no-dev-splitting.md)

**usrmerge**: `/bin`, `/sbin`, and `/lib` are all symlinks to their counterparts under `/usr/`. All parts must install files under `/usr/`.

**sbin merged into bin**: `/sbin` and `/usr/sbin` both resolve to `/usr/bin`. There is no separate `sbin` directory. Root-only tools live in `/usr/bin` like everything else; privilege is enforced by permissions, not path.

**lib64 handling**: On musl systems, `/lib64` is a symlink to `/usr/lib`. On glibc multi-arch systems, `/lib64` may point to `/usr/lib64`.

**No /usr/local pollution**: `/usr/local/` is reserved for software manually compiled and installed by the user. Parts managed by Wright must never install files there.

## Part File Installation Paths

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
| Logs | `/var/log/{partname}/` | Log directory |

## Configuration File Protection

Declare configuration files in the `backup` field of `[[output]]`:

```toml
[[output]]
backup = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

## runit Service Layout

Parts providing daemons must include a runit service directory:

```
/etc/sv/{service}/run       # Required, service start script
/etc/sv/{service}/finish    # Optional, cleanup script
/etc/sv/{service}/logs/run  # Recommended, logging script
```

Services are **NOT enabled by default**. Users enable services via symlinks:

```sh
ln -s /etc/sv/nginx /var/service/
```
