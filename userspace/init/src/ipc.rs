use crate::{call, exit};
use arkhe_abi::{Error, MAX_MESSAGE_BYTES, Syscall};

fn expect(actual: i64, expected: i64) {
    if actual != expected {
        exit(20);
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
fn close(handle: u64) {
    expect(call(Syscall::Close as u64, handle, 0, 0), 0);
}
fn sleep(ticks: u64) {
    expect(call(Syscall::SleepTicks as u64, ticks, 0, 0), 0);
}
fn read_payload(receiver: u64, from: u64, buffer: &mut [u8; MAX_MESSAGE_BYTES]) {
    expect(receive(receiver, buffer), MAX_MESSAGE_BYTES as i64);
    if buffer.iter().any(|byte| *byte != from as u8) {
        exit(21);
    }
}
fn empty_send(sender: u64) {
    expect(call(Syscall::Send as u64, sender, u64::MAX, 0), 0);
}
fn empty_receive(receiver: u64) {
    expect(call(Syscall::Receive as u64, receiver, u64::MAX, 0), 0);
}

pub fn check(console: u64, role: u64, sender: u64, receiver: u64) {
    let mut buffer = [0; MAX_MESSAGE_BYTES];
    let payload = [role as u8; MAX_MESSAGE_BYTES];
    for bad in [u64::MAX, console, receiver] {
        let expected = if bad == u64::MAX {
            Error::InvalidHandle
        } else {
            Error::PermissionDenied
        };
        expect(send(bad, &payload), expected as i64);
    }
    for bad in [u64::MAX, console, sender] {
        let expected = if bad == u64::MAX {
            Error::InvalidHandle
        } else {
            Error::PermissionDenied
        };
        expect(receive(bad, &mut buffer), expected as i64);
    }
    for (pointer, length) in [
        (0x10000, 1),
        (u64::MAX - 3, 8),
        (buffer.as_ptr() as u64, 65),
        (buffer.as_ptr() as u64, u64::MAX),
    ] {
        expect(
            call(Syscall::Send as u64, sender, pointer, length),
            Error::InvalidArgument as i64,
        );
    }
    for (pointer, length) in [
        (0x10000, 1),
        (u64::MAX - 3, 8),
        (0xffff_ffff_8000_0000, 1),
        (super::_start as *const () as u64, 1),
        (0x7000_0000 - 1, 2),
        (buffer.as_ptr() as u64, 65),
    ] {
        expect(
            call(Syscall::Receive as u64, receiver, pointer, length),
            Error::InvalidArgument as i64,
        );
    }
    expect(call(Syscall::SleepTicks as u64, 0, 0, 0), 0);
    expect(
        call(Syscall::SleepTicks as u64, 1001, 0, 0),
        Error::InvalidArgument as i64,
    );
    expect(
        call(Syscall::SleepTicks as u64, u64::MAX, 0, 0),
        Error::InvalidArgument as i64,
    );

    if env!("ARKHE_TEST_CASE") == "ipc-peer-fault" {
        if role == 2 {
            sleep(3);
            // SAFETY: Deliberate #UD verifies cleanup of a peer's live endpoints.
            unsafe {
                core::arch::asm!("ud2", options(noreturn));
            }
        }
        expect(receive(receiver, &mut buffer), Error::PeerClosed as i64);
        if role == 1 {
            expect(send(sender, &payload), Error::PeerClosed as i64);
        }
    } else {
        match role {
            0 => {
                read_payload(receiver, 2, &mut buffer);
                expect(send(sender, &payload), MAX_MESSAGE_BYTES as i64);
                empty_send(sender);
                empty_receive(receiver);
            }
            1 => {
                sleep(3);
                expect(send(sender, &payload), MAX_MESSAGE_BYTES as i64);
                read_payload(receiver, 0, &mut buffer);
                empty_receive(receiver);
                empty_send(sender);
            }
            2 => {
                sleep(5);
                expect(
                    receive(receiver, &mut buffer[..1]),
                    Error::BufferTooSmall as i64,
                );
                read_payload(receiver, 1, &mut buffer);
                expect(send(sender, &payload), MAX_MESSAGE_BYTES as i64);
                empty_receive(receiver);
                empty_send(sender);
            }
            _ => exit(22),
        }
    }
    close(sender);
    close(receiver);
    expect(send(sender, &payload), Error::InvalidHandle as i64);
    expect(receive(receiver, &mut buffer), Error::InvalidHandle as i64);
    let text = b"ipc: native message/wait/close passed\n";
    expect(
        call(
            Syscall::DebugWrite as u64,
            console,
            text.as_ptr() as u64,
            text.len() as u64,
        ),
        text.len() as i64,
    );
}
