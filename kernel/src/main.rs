#![no_std]
#![no_main]

mod arch;
mod boot;
mod user;

// The entry stack belongs to the kernel image, so the CR3 handoff can discard
// every bootloader mapping without retaining a live Limine stack frame.
core::arch::global_asm!(
    ".section .bss",
    ".balign 4096",
    ".global arkhe_boot_guard",
    "arkhe_boot_guard:",
    ".skip 4096",
    ".global arkhe_boot_stack",
    "arkhe_boot_stack:",
    ".skip 262144",
    "arkhe_boot_stack_top:",
    ".section .text",
    ".global _start",
    "_start:",
    "cli",
    "lea rsp, [rip + arkhe_boot_stack_top]",
    "xor ebp, ebp",
    "call arkhe_boot",
    "ud2",
);

use arch::x86_64 as cpu;
use arkhe_abi::{Error, Message};
use arkhe_kernel_core::capability::{CapabilityTable, ObjectId, Rights};
use arkhe_kernel_core::ipc::MessageQueue;

#[unsafe(no_mangle)]
pub extern "C" fn arkhe_boot() -> ! {
    cpu::disable_interrupts();
    cpu::init_console();
    cpu::exceptions::init();
    cpu::write_console(format_args!("arkheOS: bootstrap\n"));
    let memory = boot::BootMemory::read().unwrap_or_else(|error| match error {
        boot::BootError::InvalidRegion(reason) => panic!("invalid memory region: {reason:?}"),
        _ => panic!("boot protocol: {error:?}"),
    });
    cpu::write_console(format_args!(
        "memory: {} regions, HHDM {:#x}\n",
        memory.ranges().len(),
        memory.direct_map_offset
    ));
    check_core().expect("kernel core self-check failed");
    cpu::exceptions::check_breakpoint();
    cpu::write_console(format_args!(
        "exceptions: GDT/TSS/IDT loaded, breakpoint restored\n"
    ));
    cpu::write_console(format_args!("ARKHE_BOOT_OK\n"));
    let archive =
        arkhe_kernel_core::initramfs::Archive::new(boot::initramfs().expect("initramfs module"))
            .expect("invalid initramfs");
    cpu::write_console(format_args!(
        "initramfs: {} files, indexed read-only\n",
        archive.file_count()
    ));
    let path = if matches!(env!("ARKHE_TEST_CASE"), "boot" | "spawn") {
        "bin/supervisor"
    } else {
        "bin/init"
    };
    let executable =
        arkhe_kernel_core::elf::Executable::parse(archive.read(path).expect("boot program"))
            .expect("invalid boot ELF");
    user::launch(&memory, executable, archive);
}

fn check_core() -> Result<(), Error> {
    let mut capabilities = CapabilityTable::<2>::new();
    let endpoint = ObjectId(1);
    let handle = capabilities.insert(endpoint, Rights::SEND)?;
    if capabilities.resolve(handle, Rights::SEND)? != endpoint
        || capabilities.resolve(handle, Rights::RECEIVE) != Err(Error::PermissionDenied)
    {
        return Err(Error::PermissionDenied);
    }
    let mut queue = MessageQueue::<2>::new();
    let message = Message::new(b"bootstrap")?;
    queue.send(message)?;
    if queue.receive()? != message {
        return Err(Error::InvalidArgument);
    }
    capabilities.close(handle)?;
    if capabilities.resolve(handle, Rights::SEND) != Err(Error::InvalidHandle) {
        return Err(Error::InvalidHandle);
    }
    Ok(())
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    cpu::disable_interrupts();
    cpu::write_console(format_args!("PANIC: {info}\n"));
    #[cfg(feature = "smoke")]
    cpu::exit_test(0x11);
    #[cfg(not(feature = "smoke"))]
    cpu::halt();
}
