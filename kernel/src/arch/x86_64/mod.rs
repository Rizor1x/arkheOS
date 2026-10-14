mod serial;

pub use serial::{init_console, write_console};

pub fn disable_interrupts() {
    // SAFETY: The bootstrap CPU runs at ring 0; no interrupt handlers are installed yet.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
}

pub fn halt() -> ! {
    disable_interrupts();
    loop {
        // SAFETY: Ring 0 permits HLT. Maskable interrupts remain disabled during bootstrap.
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

#[cfg(feature = "smoke")]
pub fn exit_test(code: u32) -> ! {
    // SAFETY: Only the QEMU smoke configuration attaches isa-debug-exit at port 0xf4.
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") 0xf4_u16, in("eax") code, options(nomem, nostack));
    }
    halt()
}
