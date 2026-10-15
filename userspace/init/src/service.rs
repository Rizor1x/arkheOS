use crate::runtime::{call, exit};
use arkhe_abi::{Error, SPAWN_GRANTS_BYTES, Syscall};

fn require(valid: bool) {
    if !valid {
        exit(1);
    }
}

fn send(handle: u64, bytes: &[u8]) -> i64 {
    call(
        Syscall::Send as u64,
        handle,
        bytes.as_ptr() as u64,
        bytes.len() as u64,
    )
}

fn receive(handle: u64, bytes: &mut [u8]) -> i64 {
    call(
        Syscall::Receive as u64,
        handle,
        bytes.as_mut_ptr() as u64,
        bytes.len() as u64,
    )
}

pub fn run(console: u64, argument: u64, root: u64, sender: u64, receiver: u64) -> ! {
    require(root == 0);
    check_authority(sender, receiver);
    match argument {
        0x600 => {
            require(sender != 0 && receiver != 0);
            let mut bytes = [0u8; 8];
            require(receive(receiver, &mut bytes) == 8 && bytes == [0, 0xff, 0x80, 1, 2, 3, 4, 5]);
            bytes.reverse();
            require(send(sender, &bytes) == 8);
            exit(42)
        }
        0x601 => {
            require(sender != 0 && receiver != 0);
            require(call(Syscall::SleepTicks as u64, 5, 0, 0) == 0);
            // SAFETY: The service-death scenario deliberately raises a user #UD.
            unsafe {
                core::arch::asm!("ud2", options(noreturn));
            }
        }
        0x603 => {
            require(sender != 0 && receiver != 0 && send(sender, &[]) == 0);
            require(receive(receiver, &mut []) == Error::PeerClosed as i64);
            require(send(sender, &[]) == 0);
        }
        0x604 => {
            require(console != 0 && sender == 0 && receiver != 0);
            require(call(Syscall::SleepTicks as u64, 5, 0, 0) == 0);
            require(receive(receiver, &mut []) == 0);
            require(receive(receiver, &mut []) == Error::PeerClosed as i64);
            let text = b"service: orphan observed parent closure\n";
            require(
                call(
                    Syscall::DebugWrite as u64,
                    console,
                    text.as_ptr() as u64,
                    text.len() as u64,
                ) == text.len() as i64,
            );
        }
        0x605 => {
            require(sender != 0 && receiver != 0);
            require(send(sender, &[]) == 0 && receive(receiver, &mut []) == 0);
        }
        0x606 => {
            require(console == 0 && sender != 0 && receiver == 0);
            require(send(sender, &[]) == 0);
        }
        0x607 => {
            require(console == 0 && sender == 0 && receiver != 0);
            require(receive(receiver, &mut []) == 0);
        }
        0x608 => {
            require(sender != 0 && receiver != 0);
            require(call(Syscall::Close as u64, receiver, 0, 0) == 0);
            require(send(sender, &[]) == 0);
        }
        _ => exit(1),
    }
    exit(0)
}

fn check_authority(sender: u64, receiver: u64) {
    let grants = [0u8; SPAWN_GRANTS_BYTES];
    let path = b"etc/motd";
    for index in 0..8 {
        let handle = (1 << 32) | index;
        for result in [
            call(
                Syscall::FileOpen as u64,
                handle,
                path.as_ptr() as u64,
                path.len() as u64,
            ),
            call(Syscall::ChannelCreate as u64, handle, 0, 16),
            call(
                Syscall::ProcessSpawnWithGrants as u64,
                handle,
                0,
                grants.as_ptr() as u64,
            ),
        ] {
            require(
                result == Error::InvalidHandle as i64 || result == Error::PermissionDenied as i64,
            );
        }
    }
    for handle in [sender, receiver] {
        let expected = if handle == 0 {
            Error::InvalidHandle
        } else {
            Error::PermissionDenied
        };
        require(call(Syscall::DebugWrite as u64, handle, 0, 0) == expected as i64);
    }
}
