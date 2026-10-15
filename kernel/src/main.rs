#![no_std]
#![no_main]

mod arch;
mod boot;
mod user;

use arch::x86_64 as cpu;
use arkhe_abi::{Error, Message};
use arkhe_kernel_core::capability::{CapabilityTable, ObjectId, Rights};
use arkhe_kernel_core::ipc::MessageQueue;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
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
    #[cfg(feature = "smoke")]
    cpu::exceptions::trigger_test_fault();
    cpu::write_console(format_args!("ARKHE_BOOT_OK\n"));
    let archive =
        arkhe_kernel_core::initramfs::Archive::new(boot::initramfs().expect("initramfs module"))
            .expect("invalid initramfs");
    cpu::write_console(format_args!(
        "initramfs: {} files, indexed read-only\n",
        archive.file_count()
    ));
    let executable =
        arkhe_kernel_core::elf::Executable::parse(archive.read("bin/init").expect("bin/init"))
            .expect("invalid init ELF");
    user::launch(
        memory.ranges(),
        memory.direct_map_offset,
        executable,
        archive,
    );
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
