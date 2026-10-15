# arkheOS

An experimental x86_64 operating system written in Rust. The project aims for a
small kernel, isolated system services, native applications, and a desktop interface.

The kernel boots through BIOS or UEFI and starts a native Rust supervisor in
ring 3. The supervisor loads programs from a read-only initramfs, waits for their
exit, and restarts a child after a deliberate fault. Every program has its own
address space and capability table. The demonstration repeats after reclaiming
all process frames and completion records.

Implemented:

- COM1 console, GDT/TSS/IDT, and dedicated exception stacks;
- validated usable RAM ranges, owned frame leases, and per-process page tables;
- single-CPU round-robin preemption, voluntary yield, timer sleep, and interrupt-driven idle;
- capability-checked message delivery, blocked receivers, and peer-close wakeups;
- channel creation through explicit authority, with transactional descriptor allocation;
- legacy PC PIC/PIT timer at approximately 100 Hz;
- RX user code, RW/NX data and stack, and unmapped stack guard pages;
- bounded USTAR initramfs with checked headers and indexed file lookup;
- static ELF64 loading with segment, entry, bounds, and W^X validation;
- version, console-write, close, yield, timer-tick, file-open/read, and exit calls
  through `int 0x80`;
- capability-controlled process creation, blocking wait, and retained exit/fault status;
- page reclamation on exit/fault and rollback of partially loaded executables;
- host tests and real kernel/user fault injection under BIOS and UEFI QEMU.

Execution uses one CPU with at most four live processes. Kernel higher-half
mappings are inherited from Limine; APIC and SMP are not implemented. Channels
can be created within a process; granting their sides to spawned children is
not implemented. Children receive only an explicitly granted console, without
namespace, channel-creation, or process-creation authority. Persistent storage,
Rust `std`, Python, and a desktop are not supported yet. The kernel is
experimental and is not ready for everyday use.

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
cargo xtask smoke-scheduler
cargo xtask smoke-ipc
cargo xtask smoke-processes
```

`check` runs formatting, Clippy, host tests, kernel/application builds, and ELF
and repository checks. `smoke` boots the kernel and supervisor, verifies child
outcomes, and requires full resource reclamation within 30 seconds.
Each userspace test runs two rounds with fresh address spaces. Smoke builds
also exhaust a small frame pool during ELF loading and require complete rollback.

`smoke-exceptions` raises real divide, page, and double faults. The double-fault
handler must execute on its independent emergency stack. Every boot also checks
register and flag restoration after a returning breakpoint.

`smoke-userspace` verifies that user code cannot read mapped kernel memory,
execute privileged instructions, write its RX code, execute its NX stack, or use
unsupported x87 state. Each fault must leave the other processes running.
The application also tests invalid/closed handles, unknown calls, pointer
overflow, unmapped buffers, guard-page crossings, and valid cross-page copies.
Each boundary-test application opens and reads text and binary files, checks independent
file positions, stale handles, descriptor exhaustion, and failed-copy rollback.
Writes into code/kernel memory are rejected; a destination crossing an unmapped
page must remain unchanged.

`smoke-scheduler` verifies timer preemption of an application that never yields,
GPR/flag restoration across interrupts, independent writable data, and continued
execution of other processes. A separate case requires timer interrupts to wake
the idle kernel after every process has exited.

`smoke-ipc` exchanges binary and empty messages between three processes, checks
small/invalid receive buffers, and sleeps with all tasks blocked. A second case
faults a peer while receivers wait; they must wake with `PeerClosed`. Every round
must release all endpoint objects and process frames. Host tests also cover queue
backpressure, failed-copy rollback, and stale endpoint generations.

`smoke-processes` checks executable authority, malformed ELF rejection, child
exit/fault status, restart, stale waits, concurrent children, retained completion
records across slot reuse, and process/descriptor exhaustion. Closing a watcher
or exiting its parent must let the child finish and release every resource.
A child without a console must receive no capabilities. These checks also run
in the normal startup demonstration, except for the malformed-image fixture.
The supervisor also tests channel creation, side rights, queue backpressure,
stale handles, full descriptor/endpoint quotas, rejected and cross-page result
buffers, and cleanup of channels left open at process exit.

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
| `userspace/init/` | Native Rust supervisor, worker, and boundary tests |
| `xtask/` | Build, image, and emulator commands |
| `boot/` | Boot configuration and pinned Limine revision |
| `tools/`, `tests/` | Validation tools and their tests |

The portable libraries are `no_std` and forbid unsafe code. Endpoint queues and
scheduling transitions are host-tested; user-copy and wakeup integration run in QEMU.

Generated files stay in `target/`, `build/`, and `.cache/`. `cargo xtask iso`
builds `build/normal/arkheOS.iso`; each smoke case uses a separate build directory.
The initramfs contains only explicitly selected files, with reproducible metadata.

Local Git checks can be installed with `cargo xtask hooks`.

## Filesystem

The current filesystem is a read-only USTAR initramfs supplied by Limine. It contains
`bin/supervisor`, `bin/worker`, `bin/init`, and `etc/motd`. Headers, checksums,
paths, bounds, and duplicate names are validated before execution. File lookup
uses a bounded sorted index; file data stays in the retained boot module without
an additional extracted copy.

Applications access the namespace through a granted directory capability. Each
open file has its own cursor, and each process has its own descriptor table.
Only read access is supported. There is no persistent disk filesystem, disk-write
path, or writable RAM filesystem yet. A disk format has not been selected;
block-device support and recovery tests come before that decision.

## Initial application ABI

Applications are static x86_64 ELF64 executables with a Rust `no_std` runtime.
The entry function receives its console handle in RDI, an application argument
in RSI, and a namespace handle in RDX. The boot supervisor receives its channel
factory handle in RCX and zero in R8. Boundary tests instead receive an outgoing
channel in RCX and an incoming channel in R8, with role numbers in RSI.
Spawned children receive only the optional console and argument; RDX, RCX, and
R8 are zero. `userspace/init` provides complete build and syscall examples.
The ABI is experimental.

`int 0x80` takes a call number in RAX and arguments in RDI, RSI, and RDX.
RAX returns a nonnegative result or a negative `arkhe-abi::Error`; other GPRs are
preserved. Hardware floating-point/SIMD instructions trap; context saving and
thread-local storage are not supported. Software floating point uses the bare Rust target defaults.

| Call | Number | Arguments and result |
| --- | --- | --- |
| Yield | 0 | Gives other runnable processes a turn; returns 0 |
| Send | 1 | Sender handle, payload pointer, length ≤64; returns bytes queued |
| Receive | 2 | Receiver handle, writable buffer pointer, capacity ≤64; blocks until a message or peer closure |
| Close | 3 | Handle; returns 0 |
| DebugWrite | 4 | Console handle, UTF-8 buffer pointer, length ≤256; returns bytes written |
| Exit | 5 | Unsigned 32-bit exit status; valid calls do not return |
| Version | 6 | Returns `ABI_VERSION` |
| ClockTicks | 7 | Returns the saturating count of delivered timer interrupts |
| FileOpen | 8 | Directory handle, UTF-8 path pointer, path length ≤100 bytes; returns a file handle |
| FileRead | 9 | File handle, writable buffer pointer, length ≤256; returns bytes read |
| SleepTicks | 10 | Relative delivered timer ticks, 0..1000; returns 0 after waking |
| ProcessSpawn | 11 | Executable file handle, argument, console handle or 0; returns a child watcher handle |
| ProcessWait | 12 | Child watcher handle, 0, 0; blocks until completion, returns status and consumes the handle |
| ChannelCreate | 13 | Channel factory handle, writable result pointer, length exactly 16; returns 0 |

File paths are relative to the granted boot namespace. Empty/absolute paths,
`.`/`..` components, repeated separators, backslashes, and control characters
are rejected. Open returns `NotFound` for absent files and `NoSpace` when the
process's eight capability slots are full. The supervisor starts with three grants;
boundary-test instances start with four. Closing a handle frees its slot.
Handles are local to a process; closed generations never become valid again.

Read returns 0 at EOF or for length zero; no destination access occurs in those
cases. Other reads validate every actual destination page for user write access
before copying anything. Failure preserves both file position and destination
bytes. Separate opens have independent cursors. Closing the namespace prevents
further opens through that handle and leaves existing file handles usable.
These calls are a native interface; POSIX/libc compatibility is not implemented.

Spawning requires an executable file capability. FileOpen grants execute rights
only for files with an execute mode bit opened through a namespace with spawn
authority. Read access alone is insufficient. These are native capability rules,
not POSIX user/group or setuid permissions. File cursor position does not affect
execution. A child gets zeroed private pages and the explicitly selected console;
no file, namespace, IPC, or spawn capability is inherited. Invalid images return
`BadExecutable`; exhausted process/descriptor/record quotas return `NoSpace`,
and frame exhaustion returns `NoMemory`. Failed creation rolls back reservations.

Wait returns a normal exit code in 0..2³²−1, or `FAULT_STATUS_BASE | vector` for
an x86 user exception. Waiting consumes the watcher exactly once; waiting again
returns `InvalidHandle`. Completed status survives execution-slot reuse until
wait or close. Closing the watcher detaches without killing the child. Parent
exit closes its watchers; children continue independently. There are eight bounded
completion records, including running children and uncollected outcomes. There
is no process kill, arbitrary capability transfer, or `fork`/`exec` interface.

ChannelCreate requires a dedicated factory capability with create authority.
It writes two little-endian u64 handles, sender first and receiver second.
The whole result must be writable user memory, including unaligned buffers and
page crossings. Invalid buffers are rejected before allocation; quota failures
publish neither side and leave the destination unchanged. Creation requires two
free capability slots and one of the three global endpoint slots. Closing the
factory prevents further creation through that handle; existing channels remain
usable. `arkhe-abi::ChannelPair` encodes and decodes this byte format.

Each channel has one sender, one receiver, and a four-message FIFO. Send is
nonblocking: a full queue returns `WouldBlock`; no receivers returns `PeerClosed`.
Receive returns the message length. Empty queues block without polling; the whole
advertised buffer is checked before waiting. A small buffer returns `BufferTooSmall`
and leaves the message queued. Copy failure also leaves the queue unchanged.
Zero-length messages are supported. A zero-length send or a receive with
capacity zero ignores its buffer pointer.

Closing the sender allows queued data to drain, then wakes receivers with
`PeerClosed`. Closing the receiver discards its queue and prevents further sends.
Exit and user faults close every owned endpoint. Slots carry nonwrapping
generations and cannot be reused through stale endpoint IDs. Boot grants form a
three-process ring; duplication, capability transfer, spawn grants for channel
sides, shared memory, and RPC reply capabilities are not implemented.

Each application has a 16 KiB stack. Exit and user faults release that process’s mappings after CR3
has switched away; the remaining runnable processes continue. The frame pool
tracks at most 4,096 frames, with at most 1,100 owned frames per address space.
Reused pages are zeroed before loading. Timer ticks are a scheduling counter,
not an accurate wall clock: interrupts can coalesce while the kernel masks IRQs.
Kernel handlers run without preemption. Blocking calls save a user continuation
and select another context; no borrowed Rust state or kernel call stack is parked
across an interrupt-enabled idle period. Idle uses a separate stack and wakes
sleeping tasks through the timer.
