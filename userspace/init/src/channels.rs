use super::{open, require};
use crate::runtime::{call, exit};
use arkhe_abi::{CHANNEL_PAIR_BYTES, ChannelPair, Error, Syscall};
use core::sync::atomic::{AtomicU8, Ordering};

#[repr(align(4096))]
struct Buffer([AtomicU8; 8192]);
static BUFFER: Buffer = Buffer([const { AtomicU8::new(0) }; 8192]);
static READ_ONLY: [u8; CHANNEL_PAIR_BYTES] = [0; CHANNEL_PAIR_BYTES];

fn create(factory: u64, pointer: u64, length: u64) -> i64 {
    call(Syscall::ChannelCreate as u64, factory, pointer, length)
}

pub(super) fn pair(console: u64, factory: u64) -> ChannelPair {
    let mut bytes = [0; CHANNEL_PAIR_BYTES];
    require(
        console,
        create(factory, bytes.as_mut_ptr() as u64, bytes.len() as u64) == 0,
        "channel: creation failed\n",
    );
    match ChannelPair::decode(&bytes) {
        Ok(pair) => pair,
        Err(_) => exit(1),
    }
}

pub(super) fn close(console: u64, handle: u64) {
    require(
        console,
        call(Syscall::Close as u64, handle, 0, 0) == 0,
        "channel: close failed\n",
    );
}

pub fn check(console: u64, root: u64, factory: u64) {
    check_buffers(console, root, factory);
    check_queue(console, factory);
    check_cap_quota(console, root, factory);
    let text = b"channel: creation, buffers and descriptor rollback verified\n";
    require(
        console,
        call(
            Syscall::DebugWrite as u64,
            console,
            text.as_ptr() as u64,
            text.len() as u64,
        ) == text.len() as i64,
        "channel: console failed\n",
    );
}

fn check_buffers(console: u64, root: u64, factory: u64) {
    let mut bytes = [0xa5; CHANNEL_PAIR_BYTES];
    for (handle, error) in [
        (0, Error::InvalidHandle),
        (u64::MAX, Error::InvalidHandle),
        (console, Error::PermissionDenied),
        (root, Error::PermissionDenied),
    ] {
        require(
            console,
            create(handle, bytes.as_mut_ptr() as u64, CHANNEL_PAIR_BYTES as u64) == error as i64
                && bytes == [0xa5; CHANNEL_PAIR_BYTES],
            "channel: authority failure changed destination\n",
        );
    }
    for length in [0, 1, 15, 17, u64::MAX] {
        require(
            console,
            create(factory, bytes.as_mut_ptr() as u64, length) == Error::InvalidArgument as i64
                && bytes == [0xa5; CHANNEL_PAIR_BYTES],
            "channel: invalid length accepted\n",
        );
    }
    for pointer in [
        0,
        0x10000,
        u64::MAX - 7,
        0xffff_ffff_8000_0000,
        super::_start as *const () as u64,
        READ_ONLY.as_ptr() as u64,
    ] {
        require(
            console,
            create(factory, pointer, CHANNEL_PAIR_BYTES as u64) == Error::InvalidArgument as i64,
            "channel: invalid destination accepted\n",
        );
    }
    let edge = 0x7000_0000u64 - 8;
    // SAFETY: The ABI's final stack bytes are above live Rust stack values; entry
    // never returns. The following page is unmapped, so no whole-result copy is valid.
    unsafe {
        for index in 0..8 {
            core::ptr::write_volatile((edge + index) as *mut u8, 0x5a);
        }
    }
    for _ in 0..12 {
        require(
            console,
            create(factory, edge, CHANNEL_PAIR_BYTES as u64) == Error::InvalidArgument as i64,
            "channel: guard crossing accepted\n",
        );
    }
    for index in 0..8 {
        // SAFETY: These same process-owned bytes remain mapped after the rejected copy.
        require(
            console,
            unsafe { core::ptr::read_volatile((edge + index) as *const u8) } == 0x5a,
            "channel: partial result crossed guard\n",
        );
    }
    let pointer = BUFFER.0[4090].as_ptr() as u64;
    require(
        console,
        create(factory, pointer, CHANNEL_PAIR_BYTES as u64) == 0,
        "channel: unaligned cross-page result failed\n",
    );
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = BUFFER.0[4090 + index].load(Ordering::Relaxed);
    }
    let pair = match ChannelPair::decode(&bytes) {
        Ok(pair) => pair,
        Err(_) => exit(1),
    };
    close(console, pair.sender.raw());
    close(console, pair.receiver.raw());
}

fn check_queue(console: u64, factory: u64) {
    let channel = pair(console, factory);
    let sender = channel.sender.raw();
    let receiver = channel.receiver.raw();
    let mut output = [0x5au8; 4];
    let target = output.as_mut_ptr() as u64;
    require(
        console,
        call(Syscall::Send as u64, receiver, 0, 0) == Error::PermissionDenied as i64
            && call(Syscall::Receive as u64, sender, target, 4) == Error::PermissionDenied as i64
            && create(sender, target, 16) == Error::PermissionDenied as i64,
        "channel: side rights escaped\n",
    );
    for index in 0..4 {
        let data: [u8; 4] = [index, 0, 0xff, 0x80];
        require(
            console,
            call(Syscall::Send as u64, sender, data.as_ptr() as u64, 4) == 4,
            "channel: queue fill failed\n",
        );
    }
    require(
        console,
        call(Syscall::Send as u64, sender, u64::MAX, 0) == Error::WouldBlock as i64
            && call(Syscall::Receive as u64, receiver, target, 3) == Error::BufferTooSmall as i64
            && output == [0x5a; 4]
            && call(Syscall::Receive as u64, receiver, u64::MAX, 4)
                == Error::InvalidArgument as i64,
        "channel: queue refusal failed\n",
    );
    close(console, sender);
    for index in 0..4 {
        require(
            console,
            call(Syscall::Receive as u64, receiver, target, 4) == 4
                && output == [index, 0, 0xff, 0x80],
            "channel: FIFO or drain failed\n",
        );
    }
    require(
        console,
        call(Syscall::Receive as u64, receiver, target, 4) == Error::PeerClosed as i64,
        "channel: sender close failed\n",
    );
    close(console, receiver);
    for _ in 0..12 {
        let next = pair(console, factory);
        require(
            console,
            next.sender.raw() != sender
                && next.receiver.raw() != receiver
                && call(Syscall::Send as u64, sender, 0, 0) == Error::InvalidHandle as i64
                && call(Syscall::Close as u64, receiver, 0, 0) == Error::InvalidHandle as i64,
            "channel: stale side revived\n",
        );
        close(console, next.receiver.raw());
        require(
            console,
            call(Syscall::Send as u64, next.sender.raw(), 0, 0) == Error::PeerClosed as i64,
            "channel: receiver close failed\n",
        );
        close(console, next.sender.raw());
    }
}

fn check_cap_quota(console: u64, root: u64, factory: u64) {
    let mut files = [0; 5];
    let mut bytes = [0xa5u8; CHANNEL_PAIR_BYTES];
    for (index, handle) in files.iter_mut().enumerate() {
        let file = open(root, "etc/motd");
        require(console, file > 0, "channel: descriptor setup failed\n");
        *handle = file as u64;
        if index >= 3 {
            for _ in 0..12 {
                require(
                    console,
                    create(factory, bytes.as_mut_ptr() as u64, bytes.len() as u64)
                        == Error::NoSpace as i64
                        && bytes == [0xa5; CHANNEL_PAIR_BYTES],
                    "channel: descriptor rollback failed\n",
                );
            }
        }
    }
    for handle in files {
        close(console, handle);
    }
    let channel = pair(console, factory);
    close(console, channel.sender.raw());
    close(console, channel.receiver.raw());
}

pub fn check_exit_cleanup(console: u64, factory: u64, image: u64) {
    let orphan_channel = pair(console, factory);
    let orphan = crate::grants::start(
        image,
        0x604,
        crate::grants::options(console, 0, orphan_channel.receiver.raw()),
    );
    require(console, orphan > 0, "channel: orphan grant failed\n");
    close(console, image);
    // Console, factory, watcher and the orphan's sender leave four slots for two pairs.
    let channels = [pair(console, factory), pair(console, factory)];
    close(console, channels[0].sender.raw());
    close(console, channels[1].receiver.raw());
    let mut bytes = [0xa5u8; CHANNEL_PAIR_BYTES];
    for _ in 0..12 {
        require(
            console,
            create(factory, bytes.as_mut_ptr() as u64, bytes.len() as u64) == Error::NoSpace as i64
                && bytes == [0xa5; CHANNEL_PAIR_BYTES],
            "channel: endpoint quota failed\n",
        );
    }
    close(console, factory);
    require(
        console,
        create(factory, bytes.as_mut_ptr() as u64, bytes.len() as u64)
            == Error::InvalidHandle as i64
            && call(Syscall::Receive as u64, channels[0].receiver.raw(), 0, 0)
                == Error::PeerClosed as i64
            && call(Syscall::Send as u64, channels[1].sender.raw(), 0, 0)
                == Error::PeerClosed as i64
            && call(Syscall::Send as u64, orphan_channel.sender.raw(), 0, 0) == 0,
        "channel: factory close revoked existing channel\n",
    );
    close(console, console);
}
