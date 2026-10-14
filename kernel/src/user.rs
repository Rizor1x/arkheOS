use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use arkhe_abi::{ABI_VERSION, Error, Handle, Syscall};
use arkhe_kernel_core::capability::{CapabilityTable, ObjectId, Rights};
use arkhe_kernel_core::elf::Executable;
use arkhe_kernel_core::memory::FrameAllocator;

use crate::arch::x86_64::{
    self as cpu,
    paging::{AddressSpace, STACK_TOP},
};

const CONSOLE: ObjectId = ObjectId(1);
const MAX_WRITE: usize = 256;

struct Process {
    space: AddressSpace,
    capabilities: CapabilityTable<4>,
}
struct ProcessStorage(UnsafeCell<Option<Process>>);
// SAFETY: There is one bootstrap process and one CPU. Its syscall gate masks IRQs;
// only the executing kernel handler can access this state until it halts or exits.
unsafe impl Sync for ProcessStorage {}
static CURRENT: ProcessStorage = ProcessStorage(UnsafeCell::new(None));
static ACTIVE: AtomicBool = AtomicBool::new(false);

core::arch::global_asm!(
    ".section .text",
    ".global arkhe_enter_user",
    "arkhe_enter_user:",
    "mov cr3, rdi",
    "push 27",
    "push rdx",
    "push 2",
    "push 35",
    "push rsi",
    "mov rdi, rcx",
    "mov ax, 27",
    "mov ds, ax",
    "mov es, ax",
    "xor eax, eax",
    "xor ebx, ebx",
    "xor ecx, ecx",
    "xor edx, edx",
    "xor esi, esi",
    "xor ebp, ebp",
    "xor r8d, r8d",
    "xor r9d, r9d",
    "xor r10d, r10d",
    "xor r11d, r11d",
    "xor r12d, r12d",
    "xor r13d, r13d",
    "xor r14d, r14d",
    "xor r15d, r15d",
    "iretq",
);

unsafe extern "C" {
    fn arkhe_enter_user(root: u64, entry: u64, stack: u64, console: u64) -> !;
}

pub fn launch(frames: &mut FrameAllocator<'_>, direct_map: u64, elf: &Executable<'_>) -> ! {
    cpu::disable_interrupts();
    assert!(
        !ACTIVE.swap(true, Ordering::AcqRel),
        "bootstrap process already active"
    );
    // SAFETY: The only allocator owns the usable map, Limine maps it via HHDM,
    // and no AP or competing page-table owner has been started.
    let space = unsafe { AddressSpace::load(frames, direct_map, elf) }.expect("user address space");
    let root = space.root();
    let mut capabilities = CapabilityTable::new();
    let console = capabilities
        .insert(CONSOLE, Rights::SEND)
        .expect("console capability");
    // SAFETY: Startup is exclusive; the static state owns the address space and table
    // for all subsequent synchronous syscalls. No Rust reference survives entry.
    unsafe {
        *CURRENT.0.get() = Some(Process {
            space,
            capabilities,
        });
    }
    cpu::write_console(format_args!(
        "process: entering Rust ELF at {:#x} in ring 3\n",
        elf.entry()
    ));
    // SAFETY: Validated RX entry, RW/NX stack, ring-3 selectors, IOPL=0/IF=0,
    // a cleared lower-half root, and zeroed GPRs establish the first user context.
    unsafe { arkhe_enter_user(root, elf.entry(), STACK_TOP - 8, console.raw()) }
}

pub fn syscall(args: [u64; 4]) -> i64 {
    if !ACTIVE.load(Ordering::Acquire) {
        return Error::InvalidArgument as i64;
    }
    // SAFETY: This is the only CPU's synchronous syscall handler with IRQs masked;
    // no user code or other handler accesses the process concurrently.
    let process = unsafe { &mut *CURRENT.0.get() }
        .as_mut()
        .expect("active process state");
    let [number, a, b, c] = args;
    let result = match Syscall::try_from(number) {
        Ok(Syscall::Version) => Ok(u64::from(ABI_VERSION)),
        Ok(Syscall::Close) => process.capabilities.close(Handle::from_raw(a)).map(|()| 0),
        Ok(Syscall::DebugWrite) => write(process, a, b, c),
        Ok(Syscall::Exit) => finish(a),
        _ => Err(Error::Unsupported),
    };
    match result {
        Ok(value) => value as i64,
        Err(error) => error as i64,
    }
}

fn write(process: &Process, handle: u64, pointer: u64, length: u64) -> Result<u64, Error> {
    if process
        .capabilities
        .resolve(Handle::from_raw(handle), Rights::SEND)?
        != CONSOLE
    {
        return Err(Error::PermissionDenied);
    }
    let length = usize::try_from(length).map_err(|_| Error::InvalidArgument)?;
    if length > MAX_WRITE {
        return Err(Error::InvalidArgument);
    }
    let mut buffer = [0; MAX_WRITE];
    process
        .space
        .copy_from_user(pointer, &mut buffer[..length])
        .map_err(|_| Error::InvalidArgument)?;
    let text = core::str::from_utf8(&buffer[..length]).map_err(|_| Error::InvalidArgument)?;
    cpu::write_console(format_args!("{text}"));
    Ok(length as u64)
}

fn finish(status: u64) -> ! {
    ACTIVE.store(false, Ordering::Release);
    cpu::write_console(format_args!("process: exited with status {status}\n"));
    #[cfg(feature = "smoke")]
    {
        if status == 0 && env!("ARKHE_TEST_CASE") == "boot" {
            cpu::write_console(format_args!("ARKHE_USER_OK\n"));
            cpu::exit_test(0x10);
        }
        cpu::exit_test(0x11);
    }
    #[cfg(not(feature = "smoke"))]
    cpu::halt();
}

pub fn fault(vector: u64, error: u64, address: u64) -> ! {
    ACTIVE.store(false, Ordering::Release);
    cpu::write_console(format_args!(
        "process: terminated after exception {vector}, error {error:#x}, address {address:#x}\n"
    ));
    #[cfg(feature = "smoke")]
    {
        let marker = match (env!("ARKHE_TEST_CASE"), vector, error) {
            ("user-kernel-read", 14, 5) => Some("ARKHE_USER_KERNEL_READ_OK"),
            ("user-cli", 13, 0) => Some("ARKHE_USER_CLI_OK"),
            ("user-write-code", 14, 7) => Some("ARKHE_USER_WRITE_CODE_OK"),
            ("user-exec-stack", 14, 0x15) => Some("ARKHE_USER_EXEC_STACK_OK"),
            _ => None,
        };
        if let Some(marker) = marker {
            cpu::write_console(format_args!(
                "kernel: retained control after user fault\n{marker}\n"
            ));
            cpu::exit_test(0x10);
        }
        cpu::exit_test(0x11);
    }
    #[cfg(not(feature = "smoke"))]
    cpu::halt();
}
