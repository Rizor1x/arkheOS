# arkheOS

An experimental x86_64 operating system written in Rust. The project aims for a
small kernel, isolated system services, native applications, and a desktop interface.

The kernel boots through BIOS or UEFI, loads a static Rust ELF from a read-only
initramfs, and executes it in ring 3. The included application exercises console
capabilities, checked system-call buffers, and process exit.

Implemented:

- COM1 console, GDT/TSS/IDT, and dedicated exception stacks;
- validated usable RAM ranges and a private user page-table root;
- RX user code, RW/NX data and stack, and unmapped stack guard pages;
- bounded USTAR initramfs with checked headers and indexed file lookup;
- static ELF64 loading with segment, entry, bounds, and W^X validation;
- version, console-write, close, and exit calls through the initial `int 0x80` ABI;
- host tests and real kernel/user fault injection under BIOS and UEFI QEMU.

Currently one process runs on one CPU. There is no timer/preemption or frame
reclamation; kernel higher-half mappings are inherited from Limine. General
untrusted workloads, persistent storage, Rust `std`, Python, and a desktop are
not supported yet. The kernel is not ready for everyday use.

## Build and run

Install Rustup, Git, Python 3.10+, a C compiler, xorriso, and QEMU.
The Rust toolchain and target are pinned in `rust-toolchain.toml`.

On macOS:

```sh
brew install qemu xorriso
```

On Ubuntu 24.04:

```sh
sudo apt-get update
sudo apt-get install build-essential git python3 xorriso qemu-system-x86 ovmf
```

```sh
git clone https://github.com/Rizor1x/arkheOS.git
cd arkheOS
cargo xtask fetch-bootloader
cargo xtask check
cargo xtask run
```

The QEMU machine uses TCG, one CPU, 256 MiB RAM, and no network interface.
Output appears in the serial console. Stop QEMU with Ctrl+C.

## Tests

```sh
cargo xtask check
cargo xtask smoke
cargo xtask smoke-exceptions
cargo xtask smoke-userspace
```

`check` runs formatting, Clippy, host tests, kernel/application builds, and ELF
and repository checks. `smoke` boots the kernel, runs the Rust application, and
requires a successful exit within 30 seconds.

`smoke-exceptions` raises real divide, page, and double faults. The double-fault
handler must execute on its independent emergency stack. Every boot also checks
register and flag restoration after a returning breakpoint.

`smoke-userspace` verifies that user code cannot read mapped kernel memory,
execute privileged instructions, write its RX code, or execute its NX stack.
The application also tests invalid/closed handles, unknown calls, pointer
overflow, unmapped buffers, guard-page crossings, and a valid cross-page copy.

For UEFI, provide a matching firmware code and variables template:

```sh
ARKHE_OVMF=/usr/share/OVMF/OVMF_CODE_4M.fd \
ARKHE_OVMF_VARS=/usr/share/OVMF/OVMF_VARS_4M.fd cargo xtask smoke
```

The same environment variables apply to the fault-test commands. Homebrew QEMU
provides `edk2-x86_64-code.fd` and `edk2-i386-vars.fd` under its `share/qemu`
directory. Each UEFI run uses a fresh variables-template copy. Secure Boot is
not implemented.

## Layout

| Path | Purpose |
| --- | --- |
| `kernel/` | Boot integration, hardware, user mappings, and syscall dispatch |
| `crates/kernel-core/` | Safe memory, capability, IPC, ELF, and initramfs logic |
| `crates/abi/` | Initial application interface types |
| `userspace/init/` | Native Rust `no_std` application and boundary tests |
| `xtask/` | Build, image, and emulator commands |
| `boot/` | Boot configuration and pinned Limine revision |
| `tools/`, `tests/` | Validation tools and their tests |

The portable libraries are `no_std` and forbid unsafe code. The IPC queue is a
kernel model; cross-process IPC and scheduling have not been implemented.

Generated files stay in `target/`, `build/`, and `.cache/`. `cargo xtask iso`
builds `build/normal/arkheOS.iso`; each smoke case uses a separate build directory.
The initramfs contains only explicitly selected files, with reproducible metadata.

Local Git checks can be installed with `cargo xtask hooks`.

## Initial application ABI

Applications are static x86_64 ELF64 executables with a Rust `no_std` runtime.
The entry function receives its console handle in RDI. `userspace/init` provides
a complete build and syscall example. The ABI is experimental.

`int 0x80` takes a call number in RAX and arguments in RDI, RSI, and RDX.
RAX returns a nonnegative result or a negative `arkhe-abi::Error`; other GPRs are
preserved. Floating-point/SIMD context and thread-local storage are not supported.

| Call | Number | Arguments and result |
| --- | --- | --- |
| Close | 3 | Handle; returns 0 |
| DebugWrite | 4 | Console handle, UTF-8 buffer pointer, length ≤256; returns bytes written |
| Exit | 5 | Exit status; does not return |
| Version | 6 | Returns `ABI_VERSION` |

Yield, Send, and Receive remain reserved and return Unsupported. The initial
program has a 16 KiB stack. On exit or fault the kernel retains control and halts;
launching another process and reclaiming its pages require the next lifecycle stage.
