use arkhe_abi::Syscall;

pub fn call(number: u64, a: u64, b: u64, c: u64) -> i64 {
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
