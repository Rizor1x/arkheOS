#![no_std]
#![no_main]

mod files;
mod ipc;

use arkhe_abi::{ABI_VERSION, Error, Syscall};
use core::sync::atomic::{AtomicU64, Ordering};

#[repr(align(4096))]
struct CrossPage([u8; 8192]);
static CROSS_PAGE: CrossPage = CrossPage(cross_page());
static BSS: AtomicU64 = AtomicU64::new(0);

const fn cross_page() -> [u8; 8192] {
    let mut bytes = [0; 8192];
    let text = b"cross-page copy\n";
    let mut i = 0;
    while i < text.len() {
        bytes[4090 + i] = text[i];
        i += 1;
    }
    bytes
}

fn call(number: u64, a: u64, b: u64, c: u64) -> i64 {
    let result: u64;
    // SAFETY: Gate 0x80 implements the arkhe ABI and preserves all registers
    // except RAX, which returns a signed status. Buffer arguments remain borrowed.
    unsafe {
        core::arch::asm!("int 0x80", inlateout("rax") number => result,
            in("rdi") a, in("rsi") b, in("rdx") c);
    }
    result as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn _start(console: u64, role: u64, root: u64, sender: u64, receiver: u64) -> ! {
    if BSS.load(Ordering::Relaxed) != 0 {
        exit(1);
    }
    BSS.store(42 + role, Ordering::Relaxed);
    if BSS.load(Ordering::Relaxed) != 42 + role {
        exit(1);
    }
    let text = b"Hello from native Rust in ring 3\n";
    if call(Syscall::Version as u64, 0, 0, 0) != i64::from(ABI_VERSION)
        || call(u64::MAX, 0, 0, 0) != Error::Unsupported as i64
        || call(
            Syscall::DebugWrite as u64,
            console,
            0xffff_8000_0000_0000,
            4,
        ) != Error::InvalidArgument as i64
        || call(
            Syscall::DebugWrite as u64,
            u64::MAX,
            text.as_ptr() as u64,
            text.len() as u64,
        ) != Error::InvalidHandle as i64
    {
        exit(1);
    }
    for (pointer, length) in [
        (text.as_ptr() as u64, u64::MAX),
        (u64::MAX - 3, 8),
        (0x10000, 4),
        (0x7000_0000 - 1, 2),
    ] {
        if call(Syscall::DebugWrite as u64, console, pointer, length)
            != Error::InvalidArgument as i64
        {
            exit(1);
        }
    }
    if call(
        Syscall::DebugWrite as u64,
        console,
        CROSS_PAGE.0.as_ptr() as u64 + 4090,
        16,
    ) != 16
    {
        exit(1);
    }
    ipc::check(console, role, sender, receiver);
    files::check(console, root);
    if env!("ARKHE_TEST_CASE") == "scheduler" {
        if role == 0 {
            loop {
                core::hint::spin_loop();
            }
        }
        let start = call(Syscall::ClockTicks as u64, 0, 0, 0);
        while call(Syscall::ClockTicks as u64, 0, 0, 0) - start < 24 {
            check_registers();
            if BSS.load(Ordering::Relaxed) != 42 + role {
                exit(1);
            }
        }
    }
    for _ in 0..16 {
        if call(Syscall::Yield as u64, 0, 0, 0) != 0 || BSS.load(Ordering::Relaxed) != 42 + role {
            exit(1);
        }
    }
    match if role == 0 {
        env!("ARKHE_TEST_CASE")
    } else {
        "boot"
    } {
        "user-kernel-read" => {
            // SAFETY: Fault injection intentionally attempts a supervisor-only read.
            unsafe {
                core::arch::asm!("mov rax, [{address}]", address = in(reg) 0xffff_ffff_8000_0000u64, out("rax") _);
            }
            exit(2);
        }
        "user-fpu" => {
            // SAFETY: Unsupported x87 state must trap instead of leaking across processes.
            unsafe {
                core::arch::asm!("fninit");
            }
            exit(2);
        }
        "user-cli" => {
            // SAFETY: Fault injection deliberately attempts a privileged instruction.
            unsafe {
                core::arch::asm!("cli");
            }
            exit(2);
        }
        "user-write-code" => {
            // SAFETY: Assembly attempts a write to RX code without creating a Rust mutable reference.
            unsafe {
                core::arch::asm!("mov byte ptr [{address}], 0", address = in(reg) _start as *const () as u64);
            }
            exit(2);
        }
        "user-exec-stack" => {
            // SAFETY: This injection jumps to the writable, non-executable user stack.
            unsafe {
                core::arch::asm!("jmp rsp", options(noreturn));
            }
        }
        _ => {}
    }
    if call(
        Syscall::DebugWrite as u64,
        console,
        text.as_ptr() as u64,
        text.len() as u64,
    ) != text.len() as i64
        || call(Syscall::Close as u64, console, 0, 0) != 0
        || call(
            Syscall::DebugWrite as u64,
            console,
            text.as_ptr() as u64,
            text.len() as u64,
        ) != Error::InvalidHandle as i64
    {
        exit(1);
    }
    exit(0)
}

fn check_registers() {
    let valid: u64;
    // SAFETY: The bounded loop makes no memory accesses. Timer entry must preserve
    // callee-saved RBX, caller-saved R8/R9, the loop counter, and carry flag.
    unsafe {
        core::arch::asm!(
            "push rbx", "mov rbx, 0x13579", "mov r8, 0x2468", "mov r9, 0xabcdef",
            "mov rcx, 2000000", "stc", "2:", "dec rcx", "jnz 2b", "setc al",
            "cmp rbx, 0x13579", "sete dl", "and al, dl",
            "cmp r8, 0x2468", "sete dl", "and al, dl",
            "cmp r9, 0xabcdef", "sete dl", "and al, dl", "pop rbx",
            out("rax") valid, out("rcx") _, out("rdx") _, out("r8") _, out("r9") _,
        );
    }
    if valid & 0xff != 1 {
        exit(1);
    }
}

fn exit(status: u64) -> ! {
    call(Syscall::Exit as u64, status, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    exit(1)
}
