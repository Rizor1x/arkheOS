use arkhe_abi::Syscall;

pub fn call(number: u64, a: u64, b: u64, c: u64) -> i64 {
    if number == Syscall::MemoryResize as u64 {
        return arkhe_abi::Error::InvalidArgument as i64;
    }
    raw_call(number, a, b, c)
}

/// # Safety
/// The caller exclusively controls the process heap. No live Rust references or
/// other Heap owner may cover memory that this call can shrink or remap.
pub unsafe fn resize_heap(pages: u64, b: u64, c: u64) -> i64 {
    raw_call(Syscall::MemoryResize as u64, pages, b, c)
}

fn raw_call(number: u64, a: u64, b: u64, c: u64) -> i64 {
    let result: u64;
    // SAFETY: Gate 0x80 implements the arkhe ABI and preserves all registers
    // except RAX, which returns a signed status. Buffer arguments remain borrowed.
    unsafe {
        core::arch::asm!("int 0x80", inlateout("rax") number => result,
            in("rdi") a, in("rsi") b, in("rdx") c);
    }
    result as i64
}

pub fn exit(status: u64) -> ! {
    call(Syscall::Exit as u64, status, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}
