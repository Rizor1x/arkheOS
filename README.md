# arkheOS

An experimental x86_64 operating system written in Rust. The project aims for a
small kernel, isolated system services, native applications, and a desktop interface.

The current implementation boots through Limine, validates usable memory ranges,
provides a serial console, installs GDT/TSS/IDT with dedicated exception stacks,
and runs self-checks for capability and message-queue models.
Process isolation, application execution, persistent storage, and a desktop
are under development. The kernel is not ready for everyday use.

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
```

`check` runs formatting, Clippy, host tests, the kernel build, and ELF and
repository checks. `smoke` boots the kernel and requires both its success marker
and the expected emulator exit code within 30 seconds.
`smoke-exceptions` raises real divide, page, and double faults. The double-fault
test verifies execution on its dedicated emergency stack; every boot also checks
that a returning breakpoint restores registers and flags.

For UEFI, provide a matching firmware code and variables template:

```sh
ARKHE_OVMF=/usr/share/OVMF/OVMF_CODE_4M.fd \
ARKHE_OVMF_VARS=/usr/share/OVMF/OVMF_VARS_4M.fd cargo xtask smoke
```

Homebrew QEMU provides `edk2-x86_64-code.fd` and `edk2-i386-vars.fd` under its
`share/qemu` directory. Each UEFI run uses a fresh copy of the variables template.
Secure Boot is not implemented.

## Layout

| Path | Purpose |
| --- | --- |
| `kernel/` | Boot integration and hardware-specific code |
| `crates/kernel-core/` | Hardware-independent memory, capability, and IPC models |
| `crates/abi/` | Application interface types under development |
| `xtask/` | Build, image, and emulator commands |
| `boot/` | Boot configuration and pinned Limine revision |
| `tools/`, `tests/` | Validation tools and their tests |

The portable libraries are `no_std` and forbid unsafe code. Privileged operations
are confined to the kernel's architecture modules. Capability and queue models
currently run within the kernel; they do not yet isolate user processes.

Generated files stay in `target/`, `build/`, and `.cache/`. `cargo xtask iso`
builds `build/normal/arkheOS.iso`; smoke images use a separate build directory.

Local Git checks can be installed with `cargo xtask hooks`.
