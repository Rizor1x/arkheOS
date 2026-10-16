mod ipc;
mod process;

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use arkhe_abi::{
    ABI_VERSION, Error, FAULT_STATUS_BASE, Handle, MAX_IO_BYTES, MAX_PATH_BYTES, Syscall,
};
use arkhe_kernel_core::capability::{CapabilityTable, Rights};
use arkhe_kernel_core::completion::{CompletionId, Completions};
use arkhe_kernel_core::elf::Executable;
use arkhe_kernel_core::endpoint::{EndpointId, Endpoints};
use arkhe_kernel_core::filesystem::{OpenFile, ReadOnlyFs};
use arkhe_kernel_core::frame_pool::FramePool;
use arkhe_kernel_core::initramfs::Archive;
#[cfg(feature = "smoke")]
use arkhe_kernel_core::memory::FrameRange;
use arkhe_kernel_core::scheduler::Scheduler;

use crate::arch::x86_64::{
    self as cpu,
    exceptions::ExceptionFrame,
    paging::{self, AddressSpace, POOL_FRAMES, STACK_TOP},
};

#[derive(Clone, Copy)]
enum Resource {
    Console,
    Child(CompletionId),
    Directory,
    File(OpenFile<'static>),
    Sender(EndpointId),
    Receiver(EndpointId),
    ChannelFactory,
}
const BOOT_PROCESSES: usize = 3;
const PROCESSES: usize = 4;
const ROUNDS: u64 = 2;

#[derive(Clone, Copy)]
enum Wait {
    Child {
        completion: CompletionId,
        handle: Handle,
    },
    Receive {
        endpoint: EndpointId,
        pointer: u64,
        capacity: usize,
    },
    Sleep {
        deadline: u64,
    },
}

struct Process {
    space: AddressSpace,
    capabilities: CapabilityTable<8, Resource>,
    context: ExceptionFrame,
    ticks: u64,
    completion: Option<CompletionId>,
    wait: Option<Wait>,
}
impl Process {
    const fn new() -> Self {
        Self {
            space: AddressSpace::new(),
            capabilities: CapabilityTable::new(),
            context: ExceptionFrame::EMPTY,
            ticks: 0,
            completion: None,
            wait: None,
        }
    }
}
struct State {
    processes: [Process; PROCESSES],
    frames: FramePool<POOL_FRAMES>,
    scheduler: Scheduler<PROCESSES>,
    endpoints: Endpoints<BOOT_PROCESSES, 4>,
    completions: Completions<8>,
    spawned: usize,
    blocks: u64,
    wakes: u64,
    idle_entries: u64,
    image: Option<Executable<'static>>,
    filesystem: Option<ReadOnlyFs<'static>>,
    kernel_root: u64,
    kernel_space: AddressSpace,
    direct_map: u64,
    round: u64,
    completed: usize,
    failed: bool,
    expected_faults: usize,
    preemptions: u64,
    yields: u64,
}
struct ProcessStorage(UnsafeCell<State>);
// SAFETY: Only the BSP runs. Interrupt gates mask IRQs throughout kernel handlers;
// no handler enables them while borrowing state. NMI/machine-check paths never access it.
unsafe impl Sync for ProcessStorage {}
static STATE: ProcessStorage = ProcessStorage(UnsafeCell::new(State {
    processes: [const { Process::new() }; PROCESSES],
    frames: FramePool::new(),
    scheduler: Scheduler::new(),
    endpoints: Endpoints::new(),
    completions: Completions::new(),
    spawned: 0,
    blocks: 0,
    wakes: 0,
    idle_entries: 0,
    image: None,
    filesystem: None,
    kernel_root: 0,
    kernel_space: AddressSpace::new(),
    direct_map: 0,
    round: 0,
    completed: 0,
    failed: false,
    expected_faults: 0,
    preemptions: 0,
    yields: 0,
}));
static INITIALIZED: AtomicBool = AtomicBool::new(false);

core::arch::global_asm!(
    ".section .text",
    ".global arkhe_enter_user",
    "arkhe_enter_user:",
    "mov cr3, rdi",
    "mov ax, 27",
    "mov ds, ax",
    "mov es, ax",
    "mov rsp, rsi",
    "jmp arkhe_restore_context",
);
unsafe extern "C" {
    fn arkhe_enter_user(root: u64, context: *const ExceptionFrame) -> !;
}

fn state() -> &'static mut State {
    // SAFETY: Called exactly once per non-reentrant BSP kernel entry with IRQs off.
    // Callers never retain this borrow across a return to user mode or idle.
    unsafe { &mut *STATE.0.get() }
}

pub fn launch(
    memory: &crate::boot::BootMemory,
    elf: Executable<'static>,
    archive: Archive<'static>,
) -> ! {
    let ranges = memory.ranges();
    let direct_map = memory.direct_map_offset;
    cpu::disable_interrupts();
    assert!(
        !INITIALIZED.swap(true, Ordering::AcqRel),
        "process state already initialized"
    );
    let (root, context) = {
        let state = state();
        state.frames.initialize(ranges).expect("usable frame pool");
        state.direct_map = direct_map;
        state.image = Some(elf);
        state.filesystem = Some(ReadOnlyFs::new(archive));
        #[cfg(feature = "smoke")]
        {
            check_load_rollback(state, ranges);
            paging::kernel::check_rollback(memory);
        }
        // SAFETY: One IRQ-masked BSP owns the pool; the boot image/module and
        // private bootstrap stack remain retained across the new supervisor root.
        unsafe { state.kernel_space.build_kernel(&mut state.frames, memory) }
            .expect("owned kernel mappings");
        state.kernel_root = state.kernel_space.root();
        // SAFETY: The validated plan retains this code, stack, ISTs and module.
        unsafe {
            paging::switch(state.kernel_root);
        }
        cpu::write_console(format_args!(
            "memory: owned kernel root, {} table frames, kernel RX/RW-NX\n",
            state.kernel_space.table_frames()
        ));
        #[cfg(feature = "smoke")]
        {
            paging::kernel::trigger_fault(memory);
            cpu::exceptions::trigger_test_fault();
        }
        spawn_round(state);
        #[cfg(feature = "smoke")]
        state.processes[0].space.check_copy_permissions();
        let slot = state.scheduler.advance().expect("first runnable process");
        cpu::timer::init();
        cpu::write_console(format_args!(
            "scheduler: isolated Rust userspace, timer ~100 Hz\n"
        ));
        let process = &state.processes[slot];

        (process.space.root(), core::ptr::addr_of!(process.context))
    };
    // SAFETY: The static frame has validated RX entry, RW/NX stack, ring-3 selectors,
    // IF=1/IOPL=0 and zeroed GPRs. No state borrow survives this assembly handoff.
    unsafe { arkhe_enter_user(root, context) }
}

#[cfg(feature = "smoke")]
fn check_load_rollback(state: &mut State, ranges: &[FrameRange]) {
    // The main pool has issued no frames; this temporary pool exclusively owns RAM
    // during the failed load and returns every lease before the main pool is used.
    assert_eq!(state.frames.live_count(), 0);
    let mut small = FramePool::<4>::new();
    small.initialize(ranges).expect("rollback test pool");
    // SAFETY: Exclusive temporary frame ownership, IRQs off, shared supervisor mappings.
    let result = unsafe {
        state.processes[0].space.load(
            &mut small,
            state.direct_map,
            state.image.as_ref().expect("init image"),
        )
    };
    assert_eq!(result, Err(paging::MapError::NoMemory));
    assert_eq!(small.live_count(), 0, "failed ELF load leaked frames");
    cpu::write_console(format_args!("memory: partial ELF load rolled back\n"));
}

fn spawn_round(state: &mut State) {
    assert_eq!(
        state.frames.live_count(),
        state.kernel_space.table_frames(),
        "previous round leaked frames"
    );
    assert_eq!(state.endpoints.live_count(), 0, "endpoint leak");
    assert_eq!(state.completions.live_count(), 0, "completion leak");
    if supervised() {
        process::launch_supervisor(state);
        state.round += 1;
        return;
    }
    let endpoints: [EndpointId; BOOT_PROCESSES] =
        core::array::from_fn(|_| state.endpoints.create().expect("endpoint quota"));
    for (role, process) in state.processes.iter_mut().take(BOOT_PROCESSES).enumerate() {
        // SAFETY: The pool exclusively owns usable RAM. The BSP has not started APs;
        // supervisor mappings remain shared, and this process is not yet runnable.
        unsafe {
            process.space.load(
                &mut state.frames,
                state.direct_map,
                state.image.as_ref().expect("init image"),
            )
        }
        .expect("user address space");
        process.capabilities = CapabilityTable::new();
        let console = process
            .capabilities
            .insert(Resource::Console, Rights::SEND)
            .expect("console capability");
        let root = process
            .capabilities
            .insert(Resource::Directory, Rights::OPEN)
            .expect("root capability");
        let sender = process
            .capabilities
            .insert(
                Resource::Sender(endpoints[(role + 1) % BOOT_PROCESSES]),
                Rights::SEND,
            )
            .expect("send capability");
        let receiver = process
            .capabilities
            .insert(Resource::Receiver(endpoints[role]), Rights::RECEIVE)
            .expect("receive capability");
        process.wait = None;
        process.context = ExceptionFrame::user(
            state.image.as_ref().expect("init image").entry(),
            STACK_TOP - 8,
            console.raw(),
            role as u64,
            root.raw(),
            sender.raw(),
            receiver.raw(),
        );
        process.ticks = 0;
        state.scheduler.insert(role).expect("vacant process slot");
    }
    state.round += 1;
}

pub fn syscall(frame: &mut ExceptionFrame) {
    let state = state();
    let slot = state.scheduler.current().expect("running syscall process");
    let [number, a, b, c] = frame.arguments();
    match Syscall::try_from(number) {
        Ok(Syscall::Exit) => {
            if a > u64::from(u32::MAX) {
                frame.result(Error::InvalidArgument as i64);
                return;
            }
            cpu::write_console(format_args!("process {slot}: exited with status {a}\n"));
            state.failed |= a != 0 && state.processes[slot].completion.is_none();
            retire(state, frame, a);
        }
        Ok(Syscall::ProcessSpawn) => {
            frame.result(status(process::spawn(state, slot, a, b, c)));
        }
        Ok(Syscall::ProcessSpawnWithGrants) => {
            frame.result(status(process::spawn_with_grants(state, slot, a, b, c)));
        }
        Ok(Syscall::ChannelCreate) => {
            frame.result(status(ipc::create(state, slot, a, b, c)));
        }
        Ok(Syscall::ProcessWait) => {
            if b != 0 || c != 0 {
                frame.result(Error::InvalidArgument as i64);
            } else {
                process::wait(state, frame, slot, Handle::from_raw(a));
            }
        }
        Ok(Syscall::Yield) => {
            frame.result(0);
            state.yields = state.yields.saturating_add(1);
            state.processes[slot].context = *frame;
            state.scheduler.advance();
            restore(state, frame);
        }
        Ok(Syscall::Send) => {
            let result = ipc::send(state, slot, a, b, c);
            frame.result(status(result));
        }
        Ok(Syscall::Receive) => {
            ipc::receive(state, frame, slot, a, b, c);
        }
        Ok(Syscall::Close) => {
            let result = state.processes[slot]
                .capabilities
                .remove(Handle::from_raw(a))
                .map(|resource| {
                    if release(state, resource) {
                        ipc::wake_receivers(state);
                    }
                    0
                });
            frame.result(status(result));
        }
        Ok(Syscall::SleepTicks) => {
            if a == 0 {
                frame.result(0);
            } else if a > 1000 || cpu::timer::ticks().checked_add(a).is_none() {
                frame.result(Error::InvalidArgument as i64);
            } else {
                frame.result(0);
                block(
                    state,
                    frame,
                    Wait::Sleep {
                        deadline: cpu::timer::ticks() + a,
                    },
                );
            }
        }
        call => {
            let process = &mut state.processes[slot];
            let result = match call {
                Ok(Syscall::Version) => Ok(u64::from(ABI_VERSION)),
                Ok(Syscall::ClockTicks) => Ok(cpu::timer::ticks().min(i64::MAX as u64)),
                Ok(Syscall::DebugWrite) => write(process, a, b, c),
                Ok(Syscall::FileOpen) => open(
                    process,
                    state.filesystem.as_ref().expect("boot filesystem"),
                    a,
                    b,
                    c,
                ),
                Ok(Syscall::FileRead) => read(process, a, b, c),
                _ => Err(Error::Unsupported),
            };
            frame.result(match result {
                Ok(value) => value as i64,
                Err(error) => error as i64,
            });
        }
    }
}

fn status(result: Result<u64, Error>) -> i64 {
    match result {
        Ok(value) => value as i64,
        Err(error) => error as i64,
    }
}

pub fn on_timer(frame: &mut ExceptionFrame) {
    let state = state();
    for slot in 0..PROCESSES {
        if matches!(state.processes[slot].wait, Some(Wait::Sleep { deadline }) if cpu::timer::ticks() >= deadline)
        {
            wake(state, slot, 0);
        }
    }
    if frame.is_user() {
        let slot = state.scheduler.current().expect("running timer process");
        state.processes[slot].ticks = state.processes[slot].ticks.saturating_add(1);
        state.preemptions = state.preemptions.saturating_add(1);
        #[cfg(feature = "smoke")]
        if env!("ARKHE_TEST_CASE") == "scheduler" && slot == 0 && state.processes[slot].ticks == 8 {
            cpu::write_console(format_args!(
                "scheduler: stopped non-cooperative test process\n"
            ));
            retire(state, frame, 0);
            return;
        }
        state.processes[slot].context = *frame;
        state.scheduler.advance();
    } else if state.scheduler.current().is_none() {
        state.scheduler.advance();
    }
    restore(state, frame);
}

fn block(state: &mut State, frame: &mut ExceptionFrame, wait: Wait) {
    let slot = state.scheduler.current().expect("blocking process");
    state.processes[slot].context = *frame;
    state.processes[slot].wait = Some(wait);
    state.blocks = state.blocks.saturating_add(1);
    state.scheduler.block_current();
    restore(state, frame);
}

fn wake(state: &mut State, slot: usize, result: i64) {
    state.processes[slot].context.result(result);
    state.processes[slot].wait = None;
    state.scheduler.wake(slot).expect("blocked wake target");
    state.wakes = state.wakes.saturating_add(1);
}

fn restore(state: &mut State, frame: &mut ExceptionFrame) {
    let root = if let Some(slot) = state.scheduler.current() {
        let process = &state.processes[slot];
        *frame = process.context;
        process.space.root()
    } else {
        *frame = ExceptionFrame::idle();
        state.idle_entries = state.idle_entries.saturating_add(1);
        state.kernel_root
    };
    if paging::active_root() != root {
        // SAFETY: All roots retain supervisor/IST mappings. Rust borrows end on
        // handler return before IRET enables IRQs on the selected user/idle stack.
        unsafe {
            paging::switch(root);
        }
    }
}

fn retire(state: &mut State, frame: &mut ExceptionFrame, outcome: u64) {
    let slot = state.scheduler.remove_current().expect("live process");
    state.completed = state
        .completed
        .checked_add(1)
        .expect("completed process count");
    state.processes[slot].wait = None;
    while let Some(resource) = state.processes[slot].capabilities.pop() {
        release(state, resource);
    }
    ipc::wake_receivers(state);
    if let Some(id) = state.processes[slot].completion.take() {
        state
            .completions
            .complete(id, outcome)
            .expect("child completion");
        process::wake_waiters(state);
    }
    if state.scheduler.current().is_none() {
        state.scheduler.advance();
    }
    let root = state
        .scheduler
        .current()
        .map_or(state.kernel_root, |next| state.processes[next].space.root());
    // SAFETY: The next process or retained owned kernel root keeps all kernel/IST mappings.
    // Switching first flushes the outgoing non-global TLB entries before reclamation.
    unsafe {
        paging::switch(root);
    }
    state.processes[slot].space.release(&mut state.frames);
    state.processes[slot].capabilities = CapabilityTable::new();
    if state.scheduler.live_count() == 0 {
        assert_eq!(state.endpoints.live_count(), 0, "endpoint leak");
        assert_eq!(state.completions.live_count(), 0, "completion leak");
        assert_eq!(
            state.frames.live_count(),
            state.kernel_space.table_frames(),
            "process frame leak"
        );
        cpu::write_console(format_args!(
            "memory: round {} reclaimed all process frames\n",
            state.round
        ));
        if state.round < ROUNDS {
            spawn_round(state);
            state.scheduler.advance();
        } else {
            finish(state);
        }
    }
    restore(state, frame);
}

fn write(process: &Process, handle: u64, pointer: u64, length: u64) -> Result<u64, Error> {
    if !matches!(
        process
            .capabilities
            .resolve(Handle::from_raw(handle), Rights::SEND)?,
        Resource::Console
    ) {
        return Err(Error::PermissionDenied);
    }
    let length = usize::try_from(length).map_err(|_| Error::InvalidArgument)?;
    if length > MAX_IO_BYTES {
        return Err(Error::InvalidArgument);
    }
    let mut buffer = [0; MAX_IO_BYTES];
    process
        .space
        .copy_from_user(pointer, &mut buffer[..length])
        .map_err(|_| Error::InvalidArgument)?;
    let text = core::str::from_utf8(&buffer[..length]).map_err(|_| Error::InvalidArgument)?;
    cpu::write_console(format_args!("{text}"));
    Ok(length as u64)
}

fn open(
    process: &mut Process,
    fs: &ReadOnlyFs<'static>,
    root: u64,
    pointer: u64,
    length: u64,
) -> Result<u64, Error> {
    if !matches!(
        process
            .capabilities
            .resolve(Handle::from_raw(root), Rights::OPEN)?,
        Resource::Directory
    ) {
        return Err(Error::PermissionDenied);
    }
    let length = usize::try_from(length).map_err(|_| Error::InvalidArgument)?;
    if length == 0 || length > MAX_PATH_BYTES {
        return Err(Error::InvalidArgument);
    }
    let mut path = [0; MAX_PATH_BYTES];
    process
        .space
        .copy_from_user(pointer, &mut path[..length])
        .map_err(|_| Error::InvalidArgument)?;
    let path = core::str::from_utf8(&path[..length]).map_err(|_| Error::InvalidArgument)?;
    let file = fs.open(path)?;
    let rights = if file.executable().is_some()
        && process
            .capabilities
            .resolve(Handle::from_raw(root), Rights::SPAWN)
            .is_ok()
    {
        Rights::READ.union(Rights::EXECUTE)
    } else {
        Rights::READ
    };
    process
        .capabilities
        .insert(Resource::File(file), rights)
        .map(Handle::raw)
}

fn read(process: &mut Process, handle: u64, pointer: u64, length: u64) -> Result<u64, Error> {
    let length = usize::try_from(length).map_err(|_| Error::InvalidArgument)?;
    let Resource::File(file) = process
        .capabilities
        .resolve_mut(Handle::from_raw(handle), Rights::READ)?
    else {
        return Err(Error::PermissionDenied);
    };
    file.read_with(length, |bytes| {
        process
            .space
            .copy_to_user(pointer, bytes)
            .map_err(|_| Error::InvalidArgument)
    })
    .map(|count| count as u64)
}

fn finish(state: &State) {
    let initial = if supervised() { 1 } else { BOOT_PROCESSES };
    assert_eq!(
        state.completed,
        state
            .spawned
            .checked_add(initial * ROUNDS as usize)
            .expect("process count")
    );
    cpu::write_console(format_args!(
        "scheduler: all processes stopped; preemptions={}, yields={}, blocks={}, wakes={}, idle={}\n",
        state.preemptions, state.yields, state.blocks, state.wakes, state.idle_entries
    ));
    #[cfg(feature = "smoke")]
    {
        let case = env!("ARKHE_TEST_CASE");
        let expected = if matches!(case, "boot" | "spawn" | "scheduler" | "idle" | "ipc") {
            0
        } else {
            ROUNDS as usize
        };
        if state.failed
            || state.expected_faults != expected
            || state.yields == 0
            || state.blocks == 0
            || state.blocks != state.wakes
            || state.idle_entries == 0
        {
            cpu::exit_test(0x11);
        }
        if case == "idle" {
            cpu::timer::begin_idle_test();
            return;
        }
        let marker = match case {
            "boot" => "ARKHE_USER_OK",
            "spawn" => "ARKHE_PROCESS_OK",
            "ipc" => "ARKHE_IPC_OK",
            "ipc-peer-fault" => "ARKHE_IPC_PEER_FAULT_OK",
            "scheduler" if state.preemptions >= 16 => "ARKHE_SCHEDULER_OK",
            "user-kernel-read" => "ARKHE_USER_KERNEL_READ_OK",
            "user-cli" => "ARKHE_USER_CLI_OK",
            "user-write-code" => "ARKHE_USER_WRITE_CODE_OK",
            "user-exec-stack" => "ARKHE_USER_EXEC_STACK_OK",
            "user-fpu" => "ARKHE_USER_FPU_OK",
            _ => {
                cpu::exit_test(0x11);
            }
        };
        cpu::write_console(format_args!("{marker}\n"));
        cpu::exit_test(0x10);
    }
}

pub fn fault(frame: &mut ExceptionFrame, vector: u64, error: u64, address: u64) {
    let state = state();
    let slot = state.scheduler.current().expect("faulting process");
    cpu::write_console(format_args!(
        "process {slot}: exception {vector}, error {error:#x}, address {address:#x}\n"
    ));
    let expected =
        (slot == 2 && env!("ARKHE_TEST_CASE") == "ipc-peer-fault" && vector == 6 && error == 0)
            || slot == 0
                && matches!(
                    (env!("ARKHE_TEST_CASE"), vector, error),
                    ("user-kernel-read", 14, 5)
                        | ("user-cli", 13, 0)
                        | ("user-write-code", 14, 7)
                        | ("user-exec-stack", 14, 0x15)
                        | ("user-fpu", 7, 0)
                );
    // Managed children report faults to their watcher; the supervisor chooses recovery.
    if state.processes[slot].completion.is_none() {
        if expected {
            state.expected_faults += 1;
        } else {
            state.failed = true;
        }
    }
    retire(state, frame, FAULT_STATUS_BASE | vector);
}

fn supervised() -> bool {
    matches!(env!("ARKHE_TEST_CASE"), "boot" | "spawn")
}

fn release(state: &mut State, resource: Resource) -> bool {
    if let Resource::Child(id) = resource {
        state.completions.detach(id).expect("owned child watcher");
        false
    } else {
        ipc::release(state, resource)
    }
}
