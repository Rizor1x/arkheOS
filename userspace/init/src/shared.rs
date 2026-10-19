use crate::channels::{close, pair};
use crate::runtime::call;
use crate::{open, require, wait};
use arkhe_abi::{
    Error, FAULT_STATUS_BASE, Handle, SHARED_MEMORY_BASE, SharedSpawn, SpawnGrants, Syscall,
};
use core::sync::atomic::{AtomicU8, Ordering};

#[repr(align(4096))]
struct Descriptor([AtomicU8; 8192]);
static DESCRIPTOR: Descriptor = Descriptor([const { AtomicU8::new(0) }; 8192]);

fn expect(console: u64, actual: i64, expected: i64) {
    require(console, actual == expected, "shared: unexpected result\n");
}

fn create(pages: u64) -> i64 {
    call(Syscall::MemoryCreate as u64, pages, 0, 0)
}
fn map(handle: u64, writable: u64) -> i64 {
    call(Syscall::MemoryMap as u64, handle, writable, 0)
}
fn unmap() -> i64 {
    call(Syscall::MemoryUnmap as u64, 0, 0, 0)
}

fn start(image: u64, argument: u64, grants: SpawnGrants, memory: u64) -> i64 {
    let bytes = SharedSpawn {
        grants,
        memory: Handle::from_raw(memory),
    }
    .encode();
    if argument == 0x700 {
        for (index, byte) in bytes.iter().enumerate() {
            DESCRIPTOR.0[4093 + index].store(*byte, Ordering::Relaxed);
        }
        return call(
            Syscall::ProcessSpawnWithMemory as u64,
            image,
            argument,
            DESCRIPTOR.0[4093].as_ptr() as u64,
        );
    }
    call(
        Syscall::ProcessSpawnWithMemory as u64,
        image,
        argument,
        bytes.as_ptr() as u64,
    )
}

fn pattern(offset: usize) -> u8 {
    (offset as u8).wrapping_mul(37).wrapping_add(11)
}

pub fn check(console: u64, root: u64, image: u64, factory: u64) {
    for pages in [0, 17, u64::MAX, 1 << 32] {
        expect(console, create(pages), Error::InvalidArgument as i64);
    }
    expect(
        console,
        call(Syscall::MemoryCreate as u64, 1, 1, 0),
        Error::InvalidArgument as i64,
    );
    expect(
        console,
        call(Syscall::MemoryCreate as u64, 1, 0, 1),
        Error::InvalidArgument as i64,
    );
    expect(console, unmap(), Error::InvalidArgument as i64);
    for handle in [0, u64::MAX, console, root, image, factory] {
        let error = if handle == 0 || handle == u64::MAX {
            Error::InvalidHandle
        } else {
            Error::PermissionDenied
        };
        expect(console, map(handle, 0), error as i64);
    }
    check_quota(console, image, root);
    for argument in 0x700..=0x706 {
        exchange(console, root, image, factory, argument);
    }
    let text = b"memory: shared buffers, rights and lifetime verified\n";
    expect(
        console,
        call(
            Syscall::DebugWrite as u64,
            console,
            text.as_ptr() as u64,
            text.len() as u64,
        ),
        text.len() as i64,
    );
}

fn check_quota(console: u64, image: u64, root: u64) {
    let mut handles = [0; 4];
    for handle in &mut handles {
        let created = create(1);
        require(console, created > 0, "shared: quota setup failed\n");
        *handle = created as u64;
    }
    expect(console, create(1), Error::NoSpace as i64);
    for handle in handles {
        close(console, handle);
    }
    let memory = create(1);
    require(console, memory > 0, "shared: recovery create failed\n");
    let memory = memory as u64;
    expect(console, map(memory, 1), SHARED_MEMORY_BASE as i64);
    if env!("ARKHE_TEST_CASE") == "spawn" {
        let invalid = open(root, "bin/invalid");
        require(console, invalid > 0, "shared: invalid fixture missing\n");
        for _ in 0..12 {
            expect(
                console,
                start(invalid as u64, 0, SpawnGrants::NONE, memory),
                Error::BadExecutable as i64,
            );
        }
        close(console, invalid as u64);
    }
    let mut files = [0; 3];
    for file in &mut files {
        let opened = open(root, "etc/motd");
        require(console, opened > 0, "shared: cap quota setup failed\n");
        *file = opened as u64;
    }
    for _ in 0..12 {
        expect(console, create(16), Error::NoSpace as i64);
        expect(
            console,
            start(image, 0, SpawnGrants::NONE, memory),
            Error::NoSpace as i64,
        );
    }
    for file in files {
        close(console, file);
    }
    expect(console, unmap(), 0);
    close(console, memory);
    expect(console, map(memory, 0), Error::InvalidHandle as i64);
}

fn exchange(console: u64, root: u64, image: u64, factory: u64, argument: u64) {
    let memory = create(3);
    require(console, memory > 0, "shared: create failed\n");
    let memory = memory as u64;
    expect(console, map(memory, 2), Error::InvalidArgument as i64);
    expect(
        console,
        call(Syscall::MemoryMap as u64, memory, 0, 1),
        Error::InvalidArgument as i64,
    );
    expect(console, map(memory, 1), SHARED_MEMORY_BASE as i64);
    expect(console, map(memory, 0), Error::NoSpace as i64);
    for offset in 0..3 * 4096 {
        let pointer = (SHARED_MEMORY_BASE + offset as u64) as *mut u8;
        // SAFETY: This freshly zeroed object has only the parent's RW/NX mapping.
        // No Rust references to its bytes are created or retained across IPC.
        unsafe {
            require(
                console,
                pointer.read_volatile() == 0,
                "shared: uninitialized page\n",
            );
            pointer.write_volatile(pattern(offset));
        }
    }
    expect(console, unmap(), 0);
    expect(console, map(memory, 0), SHARED_MEMORY_BASE as i64);
    let file = open(root, "etc/motd");
    require(console, file > 0, "shared: file setup failed\n");
    expect(
        console,
        call(Syscall::FileRead as u64, file as u64, SHARED_MEMORY_BASE, 1),
        Error::InvalidArgument as i64,
    );
    let mut prefix = [0; 1];
    expect(
        console,
        call(
            Syscall::FileRead as u64,
            file as u64,
            prefix.as_mut_ptr() as u64,
            1,
        ),
        1,
    );
    require(
        console,
        prefix == *b"a",
        "shared: failed copy advanced cursor\n",
    );
    // SAFETY: The retained mapping is RO; the rejected copy must preserve its bytes.
    unsafe {
        require(
            console,
            (SHARED_MEMORY_BASE as *const u8).read_volatile() == pattern(0),
            "shared: failed copy wrote data\n",
        );
    }
    close(console, file as u64);
    expect(console, unmap(), 0);
    expect(console, map(memory, 1), SHARED_MEMORY_BASE as i64);
    let channel = pair(console, factory);
    let grants = if argument == 0x706 {
        crate::grants::options(0, 0, channel.receiver.raw())
    } else {
        crate::grants::options(0, channel.sender.raw(), 0)
    };
    for bad in [0, u64::MAX, 0x7000_0000 - 16] {
        expect(
            console,
            call(Syscall::ProcessSpawnWithMemory as u64, image, argument, bad),
            Error::InvalidArgument as i64,
        );
    }
    expect(
        console,
        start(image, argument, grants, console),
        Error::PermissionDenied as i64,
    );
    expect(
        console,
        start(image, argument, grants, u64::MAX),
        Error::InvalidHandle as i64,
    );
    let child = start(image, argument, grants, memory);
    require(console, child > 0, "shared: spawn failed\n");
    close(console, memory);
    if argument == 0x706 {
        expect(console, unmap(), 0);
        close(console, channel.sender.raw());
    } else if argument == 0x700 {
        let mut digest = [0; 8];
        expect(
            console,
            call(
                Syscall::Receive as u64,
                channel.receiver.raw(),
                digest.as_mut_ptr() as u64,
                8,
            ),
            8,
        );
        let expected: u64 = (17..8210).map(|offset| u64::from(pattern(offset))).sum();
        require(
            console,
            u64::from_le_bytes(digest) == expected,
            "shared: data mismatch\n",
        );
    }
    expect(
        console,
        wait(child as u64),
        if (0x701..=0x705).contains(&argument) {
            (FAULT_STATUS_BASE | 14) as i64
        } else {
            0
        },
    );
    if argument != 0x706 {
        // SAFETY: The child has exited; the closed handle did not revoke our retained mapping.
        unsafe {
            require(
                console,
                (SHARED_MEMORY_BASE as *const u8).read_volatile() == pattern(0),
                "shared: premature release\n",
            );
        }
        expect(console, unmap(), 0);
        close(console, channel.receiver.raw());
    }
    expect(console, unmap(), Error::InvalidArgument as i64);
}
