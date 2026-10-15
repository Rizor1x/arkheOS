use crate::{call, exit};
use arkhe_abi::{Error, MAX_IO_BYTES, MAX_PATH_BYTES, Syscall};
use core::sync::atomic::{AtomicU8, Ordering};

#[repr(align(4096))]
struct Buffer([AtomicU8; 8192]);
static BUFFER: Buffer = Buffer([const { AtomicU8::new(0) }; 8192]);
#[repr(align(4096))]
struct Path([u8; 8192]);
static PATH: Path = Path(path());

const fn path() -> [u8; 8192] {
    let mut data = [0; 8192];
    let name = b"etc/motd";
    let mut index = 0;
    while index < name.len() {
        data[4092 + index] = name[index];
        index += 1;
    }
    data
}

fn expect(actual: i64, expected: i64) {
    if actual != expected {
        exit(10);
    }
}
fn open(root: u64, path: &[u8]) -> i64 {
    call(
        Syscall::FileOpen as u64,
        root,
        path.as_ptr() as u64,
        path.len() as u64,
    )
}
fn read(file: u64, pointer: u64, length: u64) -> i64 {
    call(Syscall::FileRead as u64, file, pointer, length)
}
fn close(handle: u64) {
    expect(call(Syscall::Close as u64, handle, 0, 0), 0);
}
fn opened(result: i64) -> u64 {
    if result < 0 {
        exit(11);
    }
    result as u64
}

pub fn check(console: u64, root: u64) {
    let destination = BUFFER.0[4093].as_ptr() as u64;
    let file = opened(open(root, b"etc/motd"));
    for (handle, expected) in [
        (u64::MAX, Error::InvalidHandle),
        (console, Error::PermissionDenied),
        (root, Error::PermissionDenied),
    ] {
        expect(read(handle, destination, 1), expected as i64);
    }
    expect(open(console, b"etc/motd"), Error::PermissionDenied as i64);
    expect(open(u64::MAX, b"etc/motd"), Error::InvalidHandle as i64);
    expect(open(file, b"etc/motd"), Error::PermissionDenied as i64);
    expect(
        call(Syscall::DebugWrite as u64, file, destination, 1),
        Error::PermissionDenied as i64,
    );
    for invalid in [
        b"".as_slice(),
        b"/etc/motd",
        b"../etc/motd",
        b"etc//motd",
        b"etc/./motd",
        b"etc/../motd",
        b"etc\\motd",
        b"etc/\0motd",
        b"\xff",
    ] {
        expect(open(root, invalid), Error::InvalidArgument as i64);
    }
    expect(open(root, b"etc/absent"), Error::NotFound as i64);
    expect(
        open(root, &[b'a'; MAX_PATH_BYTES + 1]),
        Error::InvalidArgument as i64,
    );
    for (pointer, length) in [
        (0x10000, 8),
        (u64::MAX - 3, 8),
        (0x7000_0000 - 1, 8),
        (0xffff_ffff_8000_0000, 8),
        (destination, u64::MAX),
    ] {
        expect(
            call(Syscall::FileOpen as u64, root, pointer, length),
            Error::InvalidArgument as i64,
        );
    }
    for (pointer, length) in [
        (0, 1),
        (0x10000, 1),
        (u64::MAX - 3, 8),
        (0xffff_ffff_8000_0000, 4),
        (super::_start as *const () as u64, 4),
        (PATH.0.as_ptr() as u64, 4),
        (destination, MAX_IO_BYTES as u64 + 1),
    ] {
        expect(read(file, pointer, length), Error::InvalidArgument as i64);
    }
    let guard_edge = 0x7000_0000 - 1;
    // SAFETY: The ABI gives this process the last writable stack page. Its final
    // byte is above the live stack and outside Rust values; the next page is unmapped.
    unsafe {
        core::ptr::write_volatile(guard_edge as *mut u8, 0x5a);
    }
    expect(read(file, guard_edge, 2), Error::InvalidArgument as i64);
    // SAFETY: The same process-owned stack byte remains mapped after the failed copy.
    if unsafe { core::ptr::read_volatile(guard_edge as *const u8) } != 0x5a {
        exit(12);
    }
    expect(read(file, u64::MAX, 0), 0);
    expect(read(file, destination, MAX_IO_BYTES as u64), 8);
    for (index, byte) in b"arkheOS\n".iter().enumerate() {
        if BUFFER.0[4093 + index].load(Ordering::Relaxed) != *byte {
            exit(13);
        }
    }
    expect(read(file, u64::MAX, 8), 0);
    close(file);
    expect(read(file, destination, 1), Error::InvalidHandle as i64);
    expect(
        call(Syscall::Close as u64, file, 0, 0),
        Error::InvalidHandle as i64,
    );

    let a = opened(open(root, b"etc/motd"));
    let b = opened(call(
        Syscall::FileOpen as u64,
        root,
        PATH.0.as_ptr() as u64 + 4092,
        8,
    ));
    if a == file {
        exit(14);
    }
    expect(read(file, destination, 1), Error::InvalidHandle as i64);
    expect(read(a, destination, 2), 2);
    expect(call(Syscall::Yield as u64, 0, 0, 0), 0);
    expect(read(b, destination, 1), 1);
    if BUFFER.0[4093].load(Ordering::Relaxed) != b'a' {
        exit(15);
    }
    expect(read(a, destination, 1), 1);
    if BUFFER.0[4093].load(Ordering::Relaxed) != b'k' {
        exit(16);
    }
    close(a);
    close(b);

    let executable = opened(open(root, b"bin/init"));
    expect(read(executable, destination, 4), 4);
    for (index, byte) in b"\x7fELF".iter().enumerate() {
        if BUFFER.0[4093 + index].load(Ordering::Relaxed) != *byte {
            exit(17);
        }
    }
    close(executable);
    let mut handles = [0; 6];
    for handle in &mut handles {
        *handle = opened(open(root, b"etc/motd"));
    }
    expect(open(root, b"etc/motd"), Error::NoSpace as i64);
    for handle in handles {
        close(handle);
    }
    let surviving = opened(open(root, b"etc/motd"));
    close(root);
    expect(open(root, b"etc/motd"), Error::InvalidHandle as i64);
    expect(read(surviving, destination, 8), 8);
    close(surviving);
    let text = b"filesystem: native open/read/close passed\n";
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
