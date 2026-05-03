# How to perform security updates on the toolchain

When a vulnerability affects the toolchain or core system libraries, you must
rebuild the affected plans in a specific order. Rebuilding out of order produces
binaries that link against the old (vulnerable) runtime.

## Required rebuild order

Run `wright apply` for each plan in this exact sequence. Do not skip steps.

1. **binutils** — assembler, linker, and ELF tools.
2. **gcc** — compiler and runtime libraries. Needs the new binutils.
3. **glibc** — C library and dynamic linker. Needs the new gcc.
4. **linux-api-headers** — kernel UAPI headers. Must match the kernel you will
   build next.
5. **linux** — the kernel itself. Needs the new headers and compiler.
6. **everything else** — userland packages, libraries, and applications. They
   must be rebuilt against the new glibc and gcc runtime.

## Why the order matters

- **binutils first**: `ld` and `as` are used by every later compile step. If you
  rebuild gcc with an old linker, the resulting compiler may produce incorrect
  relocations when it is later used to build glibc.
- **gcc before glibc**: glibc contains assembly and inline code that is compiled
  by gcc. You want the compiler that builds glibc to be the same compiler that
  will later build userland, so ABI assumptions match.
- **glibc before userland**: Every userland binary links against `libc.so` and
  `ld-linux.so`. If you rebuild an application against an old glibc, it may
  pick up the vulnerable runtime at link time or load the old `ld-linux` at
  runtime.
- **headers before kernel**: The kernel build uses `linux-api-headers` to
  validate system-call tables and UAPI constants. Mismatched headers produce
  kernel modules that refuse to load.
- **kernel before userland**: Some userland tools (e.g. `perf`, `iptables`,
  `wireguard-tools`) compile against kernel-internal headers or expect
  `uname -r` to match the running kernel.

## One-shot command

If you keep the toolchain plans in a dedicated directory, you can run the
sequence in a single shell loop:

```bash
for plan in binutils gcc glibc linux-api-headers linux; do
    wright apply "$plan"
done

# After the toolchain is updated, rebuild the rest of the system.
wright apply @base
```

Wright's `apply` command will automatically remove the old parts produced by
each plan and install the new ones, because the database now tracks which
`plan_name` each installed part belongs to.

## Verifying the update

After the rebuild finishes, confirm that the running system uses the new
binaries:

```bash
wright list --long | grep -E '(binutils|gcc|glibc|linux)'
ldd --version          # shows glibc version
uname -r               # shows kernel version
```

If any plan fails to build, fix the failure before continuing. Do not proceed
with the next plan in the chain until the current one succeeds.
