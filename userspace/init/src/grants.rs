use crate::channels::{close, pair};
use crate::runtime::call;
use crate::{open, require, spawn, wait};
use arkhe_abi::{
    ChannelPair, Error, FAULT_STATUS_BASE, Handle, SPAWN_GRANTS_BYTES, SpawnGrants, Syscall,
};
use core::sync::atomic::{AtomicU8, Ordering};

#[repr(align(4096))]
struct Buffer([AtomicU8; 8192]);
static BUFFER: Buffer = Buffer([const { AtomicU8::new(0) }; 8192]);
static READ_ONLY: [u8; SPAWN_GRANTS_BYTES] = [0; SPAWN_GRANTS_BYTES];

pub fn options(console: u64, sender: u64, receiver: u64) -> SpawnGrants {
    SpawnGrants {
        console: Handle::from_raw(console),
        sender: Handle::from_raw(sender),
        receiver: Handle::from_raw(receiver),
    }
}

fn spawn_at(image: u64, argument: u64, pointer: u64) -> i64 {
    call(
        Syscall::ProcessSpawnWithGrants as u64,
        image,
        argument,
        pointer,
    )
}

pub fn start(image: u64, argument: u64, grants: SpawnGrants) -> i64 {
    let bytes = grants.encode();
    spawn_at(image, argument, bytes.as_ptr() as u64)
}

fn started(console: u64, image: u64, argument: u64, grants: SpawnGrants) -> u64 {
    let child = start(image, argument, grants);
    require(console, child > 0, "grants: spawn failed\n");
    child as u64
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

fn prime(console: u64, channel: ChannelPair) {
    require(
        console,
        send(channel.sender.raw(), b"\0\xff\x80x") == 4,
        "grants: queue setup failed\n",
    );
}

fn drain(console: u64, channel: ChannelPair) {
    let mut bytes = [0u8; 4];
    require(
        console,
        receive(channel.receiver.raw(), &mut bytes) == 4 && bytes == *b"\0\xff\x80x",
        "grants: rejected spawn changed queued data\n",
    );
}

fn live(console: u64, channel: ChannelPair) {
    drain(console, channel);
    prime(console, channel);
}

fn moved(console: u64, handle: u64) {
    require(
        console,
        call(Syscall::Close as u64, handle, 0, 0) == Error::InvalidHandle as i64,
        "grants: parent retained moved side\n",
    );
}

fn loopback(console: u64, image: u64, channel: ChannelPair) {
    drain(console, channel);
    let child = started(
        console,
        image,
        0x605,
        options(0, channel.sender.raw(), channel.receiver.raw()),
    );
    moved(console, channel.sender.raw());
    moved(console, channel.receiver.raw());
    require(console, wait(child) == 0, "grants: moved loopback failed\n");
}

pub fn check(console: u64, root: u64, image: u64, factory: u64) {
    check_rejections(console, root, image, factory);
    check_cap_quota(console, root, image, factory);
    close(console, root);
    check_process_quota(console, image, factory);
    for argument in [0x600, 0x601, 0x600, 0x603, 0x608] {
        check_exchange(console, image, factory, argument);
    }
    check_one_side(console, image, factory);
    let text = b"grants: child exchange, rollback and peer death verified\n";
    require(
        console,
        call(
            Syscall::DebugWrite as u64,
            console,
            text.as_ptr() as u64,
            text.len() as u64,
        ) == text.len() as i64,
        "grants: console failed\n",
    );
}

fn check_rejections(console: u64, root: u64, image: u64, factory: u64) {
    let channel = pair(console, factory);
    prime(console, channel);
    let sender = channel.sender.raw();
    let receiver = channel.receiver.raw();
    for pointer in [
        0,
        0x10000,
        u64::MAX - 7,
        0xffff_ffff_8000_0000,
        0x7000_0000 - 16,
    ] {
        require(
            console,
            spawn_at(image, 0, pointer) == Error::InvalidArgument as i64,
            "grants: invalid descriptor accepted\n",
        );
        live(console, channel);
    }
    let prefix = options(console, sender, receiver).encode();
    let edge = 0x7000_0000u64 - 16;
    // SAFETY: These final stack bytes are above live Rust values and entry never
    // returns. Only the mapped prefix is written; the descriptor's last page is absent.
    unsafe {
        for (index, byte) in prefix[..16].iter().enumerate() {
            core::ptr::write_volatile((edge + index as u64) as *mut u8, *byte);
        }
    }
    for _ in 0..12 {
        require(
            console,
            spawn_at(image, 0, edge) == Error::InvalidArgument as i64,
            "grants: truncated descriptor moved a side\n",
        );
        live(console, channel);
    }
    for (grants, error) in [
        (options(console, sender, u64::MAX), Error::InvalidHandle),
        (options(console, u64::MAX, receiver), Error::InvalidHandle),
        (options(console, receiver, sender), Error::PermissionDenied),
        (options(console, sender, sender), Error::InvalidArgument),
        (options(sender, 0, receiver), Error::PermissionDenied),
        (options(console, console, receiver), Error::PermissionDenied),
        (options(console, sender, root), Error::PermissionDenied),
        (options(console, factory, receiver), Error::PermissionDenied),
        (options(console, image, receiver), Error::PermissionDenied),
    ] {
        require(
            console,
            start(image, 0, grants) == error as i64,
            "grants: authority rejection failed\n",
        );
        live(console, channel);
    }
    require(
        console,
        spawn_at(console, 0, u64::MAX) == Error::PermissionDenied as i64
            && spawn_at(u64::MAX, 0, u64::MAX) == Error::InvalidHandle as i64,
        "grants: executable authority failed\n",
    );
    check_image_rejection(console, root, channel);
    let child = spawn_at(image, 0x300, READ_ONLY.as_ptr() as u64);
    require(
        console,
        child > 0 && wait(child as u64) == 0,
        "grants: read-only empty descriptor failed\n",
    );
    drain(console, channel);
    let bytes = options(console, sender, receiver).encode();
    for (index, byte) in bytes.iter().enumerate() {
        BUFFER.0[4089 + index].store(*byte, Ordering::Relaxed);
    }
    let child = spawn_at(image, 0x605, BUFFER.0[4089].as_ptr() as u64);
    require(
        console,
        child > 0 && wait(child as u64) == 0,
        "grants: cross-page descriptor failed\n",
    );
    moved(console, sender);
    moved(console, receiver);
    require(
        console,
        start(image, 0, options(0, sender, receiver)) == Error::InvalidHandle as i64,
        "grants: stale transfer accepted\n",
    );
}

fn check_image_rejection(console: u64, root: u64, channel: ChannelPair) {
    let grants = options(0, channel.sender.raw(), channel.receiver.raw());
    let data = open(root, "etc/motd");
    require(
        console,
        data > 0 && start(data as u64, 0, grants) == Error::PermissionDenied as i64,
        "grants: read-only file executed\n",
    );
    close(console, data as u64);
    if env!("ARKHE_TEST_CASE") == "spawn" {
        let invalid = open(root, "bin/invalid");
        require(console, invalid > 0, "grants: invalid fixture missing\n");
        for _ in 0..12 {
            require(
                console,
                start(invalid as u64, 0, grants) == Error::BadExecutable as i64,
                "grants: bad image consumed handles\n",
            );
            live(console, channel);
        }
        close(console, invalid as u64);
    }
}

fn check_cap_quota(console: u64, root: u64, image: u64, factory: u64) {
    let channel = pair(console, factory);
    prime(console, channel);
    let files = [open(root, "etc/motd"), open(root, "etc/motd")];
    require(
        console,
        files.iter().all(|handle| *handle > 0),
        "grants: quota setup failed\n",
    );
    for _ in 0..12 {
        require(
            console,
            start(
                image,
                0,
                options(0, channel.sender.raw(), channel.receiver.raw()),
            ) == Error::NoSpace as i64,
            "grants: descriptor rollback failed\n",
        );
        live(console, channel);
    }
    for handle in files {
        close(console, handle as u64);
    }
    loopback(console, image, channel);
}

fn check_process_quota(console: u64, image: u64, factory: u64) {
    let channel = pair(console, factory);
    prime(console, channel);
    let mut children = [0u64; 3];
    for child in &mut children {
        let handle = spawn(image, 0x500, console);
        require(console, handle > 0, "grants: process quota setup failed\n");
        *child = handle as u64;
    }
    for _ in 0..12 {
        require(
            console,
            start(
                image,
                0,
                options(0, channel.sender.raw(), channel.receiver.raw()),
            ) == Error::NoSpace as i64,
            "grants: process rollback failed\n",
        );
        live(console, channel);
    }
    for child in children {
        require(console, wait(child) == 0, "grants: quota child failed\n");
    }
    loopback(console, image, channel);
}

fn check_exchange(console: u64, image: u64, factory: u64, argument: u64) {
    let request = pair(console, factory);
    let reply = pair(console, factory);
    let child = started(
        console,
        image,
        argument,
        options(console, reply.sender.raw(), request.receiver.raw()),
    );
    moved(console, reply.sender.raw());
    moved(console, request.receiver.raw());
    match argument {
        0x600 => {
            let input: [u8; 8] = [0, 0xff, 0x80, 1, 2, 3, 4, 5];
            let mut output = [0u8; 8];
            require(
                console,
                send(request.sender.raw(), &input) == 8
                    && receive(reply.receiver.raw(), &mut output) == 8
                    && output == [5, 4, 3, 2, 1, 0x80, 0xff, 0]
                    && wait(child) == 42,
                "grants: binary request/reply failed\n",
            );
        }
        0x601 => require(
            console,
            receive(reply.receiver.raw(), &mut []) == Error::PeerClosed as i64
                && wait(child) == (FAULT_STATUS_BASE | 6) as i64,
            "grants: fault did not wake parent\n",
        ),
        0x603 => {
            require(
                console,
                receive(reply.receiver.raw(), &mut []) == 0
                    && call(Syscall::SleepTicks as u64, 3, 0, 0) == 0,
                "grants: receive readiness failed\n",
            );
            close(console, request.sender.raw());
            require(
                console,
                receive(reply.receiver.raw(), &mut []) == 0 && wait(child) == 0,
                "grants: parent close did not wake child\n",
            );
        }
        0x608 => require(
            console,
            receive(reply.receiver.raw(), &mut []) == 0 && wait(child) == 0,
            "grants: receiver close failed\n",
        ),
        _ => crate::runtime::exit(1),
    }
    if argument != 0x603 {
        require(
            console,
            send(request.sender.raw(), &[]) == Error::PeerClosed as i64,
            "grants: child receiver leaked\n",
        );
        close(console, request.sender.raw());
    }
    require(
        console,
        receive(reply.receiver.raw(), &mut []) == Error::PeerClosed as i64,
        "grants: child sender leaked\n",
    );
    close(console, reply.receiver.raw());
}

fn check_one_side(console: u64, image: u64, factory: u64) {
    let channel = pair(console, factory);
    let child = started(console, image, 0x606, options(0, channel.sender.raw(), 0));
    moved(console, channel.sender.raw());
    require(
        console,
        receive(channel.receiver.raw(), &mut []) == 0
            && wait(child) == 0
            && receive(channel.receiver.raw(), &mut []) == Error::PeerClosed as i64,
        "grants: sender-only child failed\n",
    );
    close(console, channel.receiver.raw());
    let channel = pair(console, factory);
    require(
        console,
        send(channel.sender.raw(), &[]) == 0,
        "grants: queued input failed\n",
    );
    let child = started(console, image, 0x607, options(0, 0, channel.receiver.raw()));
    moved(console, channel.receiver.raw());
    require(
        console,
        wait(child) == 0 && send(channel.sender.raw(), &[]) == Error::PeerClosed as i64,
        "grants: receiver-only child failed\n",
    );
    close(console, channel.sender.raw());
}
