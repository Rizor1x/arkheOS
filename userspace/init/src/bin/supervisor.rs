#![no_std]
#![no_main]

#[path = "../channels.rs"]
mod channels;
#[path = "../runtime.rs"]
mod runtime;
use arkhe_abi::{Error, FAULT_STATUS_BASE, Syscall};
use runtime::{call, exit};

fn require(console: u64, valid: bool, message: &str) {
    if !valid {
        call(
            Syscall::DebugWrite as u64,
            console,
            message.as_ptr() as u64,
            message.len() as u64,
        );
        exit(1);
    }
}

fn open(root: u64, path: &str) -> i64 {
    call(
        Syscall::FileOpen as u64,
        root,
        path.as_ptr() as u64,
        path.len() as u64,
    )
}

fn spawn(image: u64, argument: u64, console: u64) -> i64 {
    call(Syscall::ProcessSpawn as u64, image, argument, console)
}

fn wait(handle: u64) -> i64 {
    call(Syscall::ProcessWait as u64, handle, 0, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn _start(console: u64, _: u64, root: u64, factory: u64) -> ! {
    channels::check(console, root, factory);
    let image = open(root, "bin/worker");
    require(console, image > 0, "supervisor: worker missing\n");
    let image = image as u64;
    check_authority(console, root, image);
    check_recovery(console, image);
    check_retained_status(console, image);
    check_process_quota(console, image);
    check_descriptor_quota(console, root, image);
    require(
        console,
        call(Syscall::Yield as u64, 0, 0, 0) == 0,
        "supervisor: yield failed\n",
    );
    let orphan = spawn(image, 0, console);
    require(console, orphan > 0, "supervisor: orphan launch failed\n");
    let text = b"supervisor: child exit, fault recovery and resource limits verified\n";
    require(
        console,
        call(
            Syscall::DebugWrite as u64,
            console,
            text.as_ptr() as u64,
            text.len() as u64,
        ) == text.len() as i64,
        "supervisor: console failed\n",
    );
    for handle in [console, root, image] {
        require(
            console,
            call(Syscall::Close as u64, handle, 0, 0) == 0,
            "supervisor: final handle close failed\n",
        );
    }
    channels::check_exit_cleanup(factory);
    exit(0)
}

fn check_authority(console: u64, root: u64, image: u64) {
    let data = open(root, "etc/motd");
    require(
        console,
        data > 0
            && spawn(data as u64, 0, console) == Error::PermissionDenied as i64
            && wait(data as u64) == Error::PermissionDenied as i64
            && call(Syscall::Close as u64, data as u64, 0, 0) == 0
            && spawn(u64::MAX, 0, console) == Error::InvalidHandle as i64
            && spawn(image, 0, image) == Error::PermissionDenied as i64
            && spawn(image, 0, u64::MAX) == Error::InvalidHandle as i64,
        "supervisor: authority checks failed\n",
    );
    if env!("ARKHE_TEST_CASE") == "spawn" {
        let invalid = open(root, "bin/invalid");
        require(
            console,
            invalid > 0,
            "supervisor: invalid fixture missing\n",
        );
        for _ in 0..12 {
            require(
                console,
                spawn(invalid as u64, 0, console) == Error::BadExecutable as i64,
                "supervisor: malformed ELF accepted\n",
            );
        }
        require(
            console,
            call(Syscall::Close as u64, invalid as u64, 0, 0) == 0,
            "supervisor: fixture close failed\n",
        );
    }
}

fn check_recovery(console: u64, image: u64) {
    let child = spawn(image, 7, console);
    require(console, child > 0, "supervisor: first spawn failed\n");
    require(
        console,
        call(Syscall::ProcessWait as u64, child as u64, 1, 0) == Error::InvalidArgument as i64
            && wait(child as u64) == 7
            && wait(child as u64) == Error::InvalidHandle as i64,
        "supervisor: wait or stale handle failed\n",
    );

    let crashed = spawn(image, 0x100, console);
    require(
        console,
        crashed > 0 && wait(crashed as u64) == (FAULT_STATUS_BASE | 6) as i64,
        "supervisor: child fault not reported\n",
    );
    let restarted = spawn(image, 0, console);
    require(
        console,
        restarted > 0 && wait(restarted as u64) == 0,
        "supervisor: restart failed\n",
    );
}

fn check_retained_status(console: u64, image: u64) {
    let retained = spawn(image, 19, console);
    require(
        console,
        retained > 0 && call(Syscall::SleepTicks as u64, 8, 0, 0) == 0,
        "supervisor: retained completion failed\n",
    );
    let reused = spawn(image, 23, console);
    require(
        console,
        reused > 0 && wait(reused as u64) == 23 && wait(retained as u64) == 19,
        "supervisor: reused slot changed exit status\n",
    );

    let detached = spawn(image, 0, console);
    require(
        console,
        detached > 0
            && call(Syscall::Close as u64, detached as u64, 0, 0) == 0
            && wait(detached as u64) == Error::InvalidHandle as i64
            && call(Syscall::SleepTicks as u64, 8, 0, 0) == 0,
        "supervisor: detached child failed\n",
    );
}

fn check_process_quota(console: u64, image: u64) {
    let mut children = [0u64; 3];
    for (index, handle) in children.iter_mut().enumerate() {
        let child = spawn(image, 0x500 + 30 + index as u64, console);
        require(console, child > 0, "supervisor: concurrent spawn failed\n");
        *handle = child as u64;
    }
    require(
        console,
        spawn(image, 0, console) == Error::NoSpace as i64,
        "supervisor: process quota failed\n",
    );
    for (index, handle) in children.into_iter().enumerate() {
        require(
            console,
            wait(handle) == 30 + index as i64,
            "supervisor: concurrent wait failed\n",
        );
    }
}

fn check_descriptor_quota(console: u64, root: u64, image: u64) {
    let mut files = [0u64; 4];
    for handle in &mut files {
        let file = open(root, "etc/motd");
        require(console, file > 0, "supervisor: descriptor setup failed\n");
        *handle = file as u64;
    }
    for _ in 0..12 {
        require(
            console,
            spawn(image, 0, console) == Error::NoSpace as i64,
            "supervisor: descriptor rollback failed\n",
        );
    }
    for handle in files {
        require(
            console,
            call(Syscall::Close as u64, handle, 0, 0) == 0,
            "supervisor: descriptor close failed\n",
        );
    }
    for argument in [0, 0x300, 0x400] {
        let child = spawn(image, argument, if argument == 0x300 { 0 } else { console });
        require(
            console,
            child > 0 && wait(child as u64) == 0,
            "supervisor: post-exhaustion launch failed\n",
        );
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    exit(1)
}
