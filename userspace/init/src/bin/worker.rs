#![no_std]
#![no_main]

#[path = "../arena.rs"]
mod arena;
#[path = "../heap.rs"]
mod heap;
#[path = "../runtime.rs"]
mod runtime;
#[path = "../service.rs"]
mod service;
#[path = "../shared_worker.rs"]
mod shared_worker;
use arkhe_abi::{Error, Syscall};
use core::sync::atomic::{AtomicU64, Ordering};
use runtime::{call, exit};

static BSS: AtomicU64 = AtomicU64::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn _start(
    console: u64,
    argument: u64,
    root: u64,
    sender: u64,
    receiver: u64,
    memory: u64,
) -> ! {
    if BSS.load(Ordering::Relaxed) != 0 {
        exit(1);
    }
    BSS.store(argument + 1, Ordering::Relaxed);
    // SAFETY: Startup has no Heap owner or references into native heap pages.
    unsafe {
        heap::check(console, argument);
        heap::leave_allocated();
    }
    if (0x700..=0x706).contains(&argument) {
        if root != 0 || console != 0 {
            exit(1);
        }
        shared_worker::run(argument, sender, receiver, memory);
    }
    if memory != 0 {
        exit(1);
    }
    if (0x600..0x700).contains(&argument) {
        service::run(console, argument, root, sender, receiver);
    }
    if root != 0 || sender != 0 || receiver != 0 {
        exit(1);
    }
    let path = b"bin/worker";
    if call(
        Syscall::FileOpen as u64,
        root,
        path.as_ptr() as u64,
        path.len() as u64,
    ) != Error::InvalidHandle as i64
        || call(Syscall::ChannelCreate as u64, console, 0, 16)
            != if console == 0 {
                Error::InvalidHandle as i64
            } else {
                Error::PermissionDenied as i64
            }
        || call(Syscall::ChannelCreate as u64, (1 << 32) | 2, 0, 16) != Error::InvalidHandle as i64
        || call(Syscall::ProcessSpawn as u64, (1 << 32) | 2, 0, 0) != Error::InvalidHandle as i64
        || call(Syscall::ProcessSpawn as u64, console, 0, 0)
            != if console == 0 {
                Error::InvalidHandle as i64
            } else {
                Error::PermissionDenied as i64
            }
    {
        exit(1);
    }
    if argument == 0x300
        && (console != 0
            || call(
                Syscall::DebugWrite as u64,
                0,
                path.as_ptr() as u64,
                path.len() as u64,
            ) != Error::InvalidHandle as i64)
    {
        exit(1);
    }
    if argument == 0x400
        && call(Syscall::Exit as u64, u64::MAX, 0, 0) != Error::InvalidArgument as i64
    {
        exit(1);
    }
    let ticks = if (0x500..0x600).contains(&argument) {
        100
    } else {
        3
    };
    if call(Syscall::SleepTicks as u64, ticks, 0, 0) != 0
        || BSS.load(Ordering::Relaxed) != argument + 1
    {
        exit(1);
    }
    if argument == 0x100 {
        // SAFETY: The supervisor test deliberately faults this child with #UD.
        unsafe {
            core::arch::asm!("ud2", options(noreturn));
        }
    }
    exit(if argument <= 255 || (0x500..0x600).contains(&argument) {
        argument & 255
    } else {
        0
    })
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    exit(1)
}
