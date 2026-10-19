use crate::runtime::{call, exit};
use arkhe_abi::{Error, SHARED_MEMORY_BASE, Syscall};

fn require(valid: bool) {
    if !valid {
        exit(1);
    }
}

pub fn run(argument: u64, sender: u64, receiver: u64, memory: u64) -> ! {
    require(memory != 0);
    require(call(Syscall::MemoryMap as u64, memory, 1, 0) == Error::PermissionDenied as i64);
    require(call(Syscall::MemoryMap as u64, memory, 0, 0) == SHARED_MEMORY_BASE as i64);
    require(call(Syscall::Close as u64, memory, 0, 0) == 0);
    require(call(Syscall::MemoryMap as u64, memory, 0, 0) == Error::InvalidHandle as i64);
    if argument == 0x706 {
        require(sender == 0 && receiver != 0);
        require(call(Syscall::Receive as u64, receiver, 0, 0) == Error::PeerClosed as i64);
    } else {
        require(sender != 0 && receiver == 0);
    }
    if matches!(argument, 0x700 | 0x706) {
        let mut digest = 0u64;
        for offset in 17..8210 {
            // SAFETY: The retained RO mapping covers this range; the parent publishes
            // bytes before spawn and does not mutate them during this protocol phase.
            let byte =
                unsafe { ((SHARED_MEMORY_BASE + offset as u64) as *const u8).read_volatile() };
            require(byte == (offset as u8).wrapping_mul(37).wrapping_add(11));
            digest += u64::from(byte);
        }
        if argument == 0x700 {
            let bytes = digest.to_le_bytes();
            require(call(Syscall::Send as u64, sender, bytes.as_ptr() as u64, 8) == 8);
            require(call(Syscall::MemoryUnmap as u64, 0, 0, 0) == 0);
        }
        // 0x706 deliberately leaves the last mapping for process teardown.
        exit(0);
    }
    let address = match argument {
        0x703 => SHARED_MEMORY_BASE - 4096,
        0x704 => SHARED_MEMORY_BASE + 3 * 4096,
        _ => SHARED_MEMORY_BASE,
    };
    if argument == 0x705 {
        require(call(Syscall::MemoryUnmap as u64, 0, 0, 0) == 0);
    }
    // SAFETY: These controlled probes deliberately violate RO/NX or read an absent
    // guard/released page. The supervisor requires a page-fault completion.
    unsafe {
        match argument {
            0x701 => core::arch::asm!("mov byte ptr [{address}], 1", address = in(reg) address),
            0x702 => {
                core::arch::asm!("jmp {address}", address = in(reg) address, options(noreturn))
            }
            _ => core::arch::asm!("mov rax, [{address}]", address = in(reg) address, out("rax") _),
        }
    }
    exit(2)
}
