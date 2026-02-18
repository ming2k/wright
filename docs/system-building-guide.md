# 系统构建指南

本文档面向使用 wright 构建和维护自定义 Linux 发行版的用户，涵盖文件系统结构规范、拆包原则和系统维护策略。

---

## 1. 文件系统层级结构 (FHS)

wright 目标发行版遵循 FHS (Filesystem Hierarchy Standard) 的精简变体，适配 musl + runit 系统。

### 1.1 理想的根目录结构

```
/
├── bin/            → 符号链接到 /usr/bin (usrmerge)
├── sbin/           → 符号链接到 /usr/sbin (usrmerge)
├── lib/            → 符号链接到 /usr/lib (usrmerge)
├── usr/
│   ├── bin/        # 所有用户和系统可执行文件
│   ├── sbin/       # 系统管理工具 (可选，也可合并到 bin/)
│   ├── lib/        # 共享库 (.so) 和内部库
│   ├── libexec/    # 程序内部使用的辅助可执行文件
│   ├── include/    # C/C++ 头文件
│   ├── share/      # 架构无关的数据文件
│   │   ├── man/    # 手册页
│   │   ├── doc/    # 文档
│   │   ├── info/   # info 页
│   │   └── locale/ # 本地化文件
│   └── local/      # 用户手动安装的软件 (不由 wright 管理)
├── etc/            # 系统配置文件
│   ├── wright/     # wright 包管理器配置
│   ├── sv/         # runit 服务定义目录
│   └── ...
├── var/
│   ├── lib/        # 程序持久状态数据
│   │   └── wright/ # wright 数据库和缓存
│   ├── log/        # 日志文件
│   ├── run/        # 运行时数据 (tmpfs)
│   ├── service/    → 指向 /etc/sv 中已启用服务的符号链接
│   ├── hold/       # Hold tree (包描述文件集合)
│   └── tmp/        # 临时文件 (重启可清除)
├── dev/            # 设备文件 (devtmpfs)
├── proc/           # 进程信息 (procfs)
├── sys/            # 内核接口 (sysfs)
├── tmp/            # 临时文件 (tmpfs，所有用户可写)
├── run/            → 符号链接到 /var/run (或独立 tmpfs)
├── boot/           # 内核和引导加载程序文件
├── home/           # 用户主目录
└── root/           # root 用户主目录
```

### 1.2 关键设计决策

**usrmerge**：`/bin`、`/sbin`、`/lib` 均为指向 `/usr/` 下对应目录的符号链接。所有包都应将文件安装到 `/usr/` 下。这简化了系统结构，避免了 `/bin` 和 `/usr/bin` 的历史分裂问题。

**无 /lib64**：musl libc 不区分 lib 和 lib64。所有库统一安装到 `/usr/lib/`。

**无 /opt**：不使用 `/opt` 目录。所有原生包安装到标准 FHS 位置。如需隔离的第三方软件，使用 Flatpak。

**无 /usr/local 污染**：`/usr/local/` 保留给用户手动编译安装的软件，wright 管理的包不应安装任何文件到该目录。

### 1.3 包内文件安装位置约定

| 文件类型 | 安装位置 | 说明 |
|----------|---------|------|
| 可执行文件 | `/usr/bin/` | 用户和系统命令统一放置 |
| 系统管理工具 | `/usr/sbin/` | 仅 root 使用的管理工具 (可选) |
| 共享库 | `/usr/lib/` | `.so` 文件和版本符号链接 |
| 静态库 | `/usr/lib/` | `.a` 文件 |
| 头文件 | `/usr/include/` | C/C++ 开发头文件 |
| pkg-config | `/usr/lib/pkgconfig/` | `.pc` 文件 |
| cmake 模块 | `/usr/lib/cmake/` | cmake 查找模块 |
| 手册页 | `/usr/share/man/` | man pages |
| 文档 | `/usr/share/doc/{pkgname}/` | README、LICENSE 等 |
| 配置文件 | `/etc/` | 系统级配置 |
| runit 服务 | `/etc/sv/{service}/` | 服务定义 |
| 运行时数据 | `/var/lib/{pkgname}/` | 数据库、状态文件等 |
| 日志 | `/var/log/{pkgname}/` | 日志文件目录 |

---

## 2. 拆包原则

### 2.1 核心理念

拆包（split package）是将一个上游源码包的构建产物拆分为多个独立的二进制包。拆包增加了复杂度，因此应当权衡利弊，只在确实有必要时才拆分。

### 2.2 拆包的判断依据

拆包的核心问题是：**同一个源码树产出的文件，是否存在使用者群体和生命周期明显不同的子集？**

判断时考虑以下因素：

1. **运行时 vs 编译时**：很多程序只需要某个库的 `.so`，不需要编译器本身
2. **体积差异**：子集体积巨大且大多数用户不需要
3. **依赖传播**：不拆分会导致大量包被迫依赖不需要的重型组件
4. **硬件/场景特异性**：固件、驱动等只对特定硬件有意义

### 2.3 典型拆包案例

#### GCC：编译器 vs 运行时库

GCC 是最典型的必须拆包案例。上游一次构建产出编译器和多个运行时库：

```
gcc (源码)
├── gcc              # C 编译器、cc1、collect2 等 (~100MB+)
├── g++              # C++ 编译器前端
├── libstdc++        # C++ 标准库运行时 (~5MB)
├── libgcc           # GCC 底层运行时 (~200KB)
├── libgomp          # OpenMP 运行时
├── libatomic        # 原子操作库
└── gcc-doc          # 文档 (info/man，体积大)
```

**为什么必须拆：**

- `libstdc++` 和 `libgcc` 是几乎所有 C++ 程序的运行时依赖。如果不拆分，安装任何 C++ 程序都会拖入完整的 GCC 编译器（100MB+），这是不可接受的
- 反过来，系统上很多包需要 `libstdc++.so`，但不需要 `g++`
- `libgomp`、`libatomic` 等小型运行时库同理——特定程序需要它们运行，但不需要编译器

```toml
# gcc/plan.toml 拆包示例
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

#### linux-firmware：按硬件拆分

linux-firmware 上游仓库包含所有硬件的固件二进制，总计超过 800MB。绝大多数用户只需要对应自己硬件的固件。

```
linux-firmware (源码, ~800MB+)
├── linux-firmware-amdgpu      # AMD GPU 固件 (~150MB)
├── linux-firmware-nvidia       # NVIDIA Nouveau 固件
├── linux-firmware-intel        # Intel 各类固件 (WiFi、GPU 等)
├── linux-firmware-iwlwifi      # Intel 无线网卡固件
├── linux-firmware-realtek      # Realtek 网卡/WiFi 固件
├── linux-firmware-ath          # Atheros/Qualcomm WiFi 固件
├── linux-firmware-broadcom     # Broadcom 固件
└── ...
```

**为什么必须拆：**

- 800MB 的固件对个人系统来说大部分是浪费——一台机器通常只需要 2-3 个子包
- 固件文件之间几乎没有依赖关系，天然适合拆分
- 按硬件厂商/类型拆分后，用户只安装自己硬件需要的固件

```toml
# linux-firmware/plan.toml 拆包示例
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

#### 更多案例速查

| 上游项目 | 拆分方式 | 拆分理由 |
|----------|---------|---------|
| **dbus** | `libdbus` + `dbus-daemon` | 许多程序链接 libdbus 但不需要守护进程本身 |
| **Python** | `python` + `python-doc` | 文档 ~50MB，运行时不需要 |
| **Mesa** | `mesa-dri` + `mesa-vulkan-intel` + `mesa-vulkan-radeon` + ... | 不同 GPU 驱动互不相关 |
| **systemd** (如适用) | `libsystemd` + `libudev` + `systemd` | 很多程序只链接 libudev，不需要 init 系统 |
| **util-linux** | `libblkid` + `libuuid` + `libmount` + `util-linux` | 库被广泛链接，工具集本身不是所有人都需要 |
| **glib** | 不拆分 | 库+工具紧耦合，体积合理，几乎总是一起使用 |
| **zlib** | 不拆分 | 小库，拆分无意义 |
| **curl** | 不拆分 | `libcurl` 和 `curl` CLI 体积都小，且经常同时需要 |

### 2.4 何时不拆包

| 情形 | 理由 |
|------|------|
| `-dev` 包（个人/小团队使用） | 磁盘空间远不如维护复杂度重要（详见 2.5） |
| 体积小的库 | 拆分后节省的空间不值得增加依赖关系复杂度 |
| 紧耦合组件 | 几乎总是一起使用的组件不应拆分 |
| 只有单一使用场景的包 | 没有不同的用户群体，拆分无受益者 |

### 2.5 关于 -dev 拆包的建议

传统发行版（Debian、Alpine）会将头文件（`.h`）、静态库（`.a`）、pkg-config（`.pc`）等开发文件拆分为 `-dev` 子包。这在以下场景是合理的：

- **大规模公共仓库**：数千用户，大多数是最终用户不需要开发文件
- **嵌入式/容器环境**：磁盘空间极度受限

但对于 wright 的目标用户（个人或小团队维护的自定义发行版），**不拆分 -dev 是更好的默认选择**：

1. **维护成本**：每个 `-dev` 拆分意味着额外的依赖声明、版本跟踪和测试
2. **构建友好**：不拆分 `-dev` 意味着安装了库就能直接编译依赖它的软件，无需额外安装 `-dev`
3. **调试友好**：头文件在故障排查时经常有用
4. **磁盘开销可忽略**：头文件和 `.pc` 文件通常只占几百 KB

**例外**：如果某个包的开发文件异常庞大（如 Qt、LLVM 的头文件超过 50MB），可以考虑拆分。

### 2.6 拆包实践

在 `plan.toml` 中使用 `[split]` 部分定义子包：

```toml
# 示例：仅拆分大体积文档
[split.doc]
description = "GCC documentation"
files = ["/usr/share/doc/gcc/*", "/usr/share/man/man7/*", "/usr/share/info/gcc*"]

# 示例：库和守护进程拆分
[split.libs]
description = "D-Bus shared libraries"
files = ["/usr/lib/libdbus-1.so*"]
dependencies = []
```

### 2.7 总结决策表

| 问题 | 回答"是" → 拆分 | 回答"否" → 不拆分 |
|------|----------------|-------------------|
| 子集文件是否有独立的使用者群体？ | 拆 | 不拆 |
| 子集文件体积是否超过主包的 30%？ | 拆 | 不拆 |
| 拆分后是否减少了至少 2 个不必要的依赖传递？ | 拆 | 不拆 |
| 子集文件在运行时是否确实不需要？ | 拆 | 不拆 |

满足两个以上"是"时再考虑拆分。

---

## 3. 依赖管理策略

### 3.1 运行时依赖的严格保护

wright 默认**禁止卸载被其他已安装包依赖的软件**。这是一个安全策略：

```
$ wright remove openssl
error: cannot remove 'openssl': required by curl, nginx, git

$ wright remove openssl --force    # 强制卸载（自行承担风险）
warning: forcing removal of openssl which is depended on by: curl, nginx, git
removed: openssl
```

这防止了意外破坏系统。如果确实需要移除，使用 `--force` 并手动处理依赖关系。

### 3.2 依赖声明原则

- **runtime**：运行时必须存在的包。仅声明直接依赖，不声明传递依赖
- **build**：仅构建时需要，不会记录到二进制包中
- **optional**：增强功能但非必须，仅作为提示信息

```toml
[dependencies]
runtime = ["zlib", "openssl >= 3.0"]   # 直接依赖
build = ["gcc", "make", "perl"]         # 仅构建时
optional = [
    { name = "nghttp2", description = "HTTP/2 support" },
]
```

### 3.3 避免循环依赖

循环依赖（A → B → A）会被 wright 的依赖解析器检测并拒绝。如果遇到上游存在循环依赖的情况：

1. 判断是否为真正的运行时循环依赖（通常不是）
2. 将其中一个方向改为 `optional` 或在 `build` 依赖中处理
3. 必要时考虑合并为一个包

---

## 4. 构建约定

### 4.1 通用编译标志

推荐的默认编译标志（在 `wright.toml` 中配置）：

```toml
[build]
cflags = "-O2 -pipe -march=x86-64"
cxxflags = "${cflags}"
```

- `-O2`：平衡优化和编译速度
- `-pipe`：使用管道而非临时文件，加快编译
- `-march=x86-64`：基准 x86_64 兼容性

### 4.2 musl 兼容性注意事项

打包时注意以下 musl 特有问题：

- 无 `execinfo.h`（backtrace 支持），可能需要 `libexecinfo` 或补丁
- `GLOB_TILDE` / `GLOB_BRACE` 不可用
- locale 支持有限
- 部分软件假设 glibc 特有的头文件存在

遇到兼容问题时，优先向上游提交补丁；如无法解决，通过 Flatpak 分发（Flatpak 使用自己的 glibc 运行时）。

### 4.3 runit 服务打包

提供守护进程的包必须包含 runit 服务目录：

```
/etc/sv/{service}/run          # 必需，服务启动脚本
/etc/sv/{service}/finish       # 可选，清理脚本
/etc/sv/{service}/log/run      # 推荐，日志脚本
```

服务**默认不启用**。用户通过符号链接启用：

```sh
ln -s /etc/sv/nginx /var/service/
```

### 4.4 配置文件保护

在 `plan.toml` 的 `[backup]` 部分声明需要保护的配置文件。这些文件在卸载时会被保留，在升级时不会被覆盖：

```toml
[backup]
files = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

---

## 5. 仓库层级与包分类

### 5.1 四级仓库结构

| 层级 | 名称 | 内容 | 更新策略 |
|------|------|------|---------|
| **core** | 核心 | 工具链、libc、内核、init、基础工具 | 极保守，仅安全修复 |
| **base** | 基础 | 网络工具、文件系统工具、常用库、wright 自身 | 稳定版本，经 core 测试后升级 |
| **extra** | 扩展 | 服务器、语言运行时、开发工具 | 跟踪稳定上游 |
| **community** | 社区 | 用户贡献包 | 无稳定性保证 |

### 5.2 不放入原生仓库的软件

- 依赖树复杂的桌面应用 → 使用 Flatpak
- 上游已停止维护的软件
- 仅支持 glibc 且无法合理打补丁的软件
- 闭源软件
