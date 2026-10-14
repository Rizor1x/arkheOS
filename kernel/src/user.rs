use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use arkhe_abi::{ABI_VERSION, Error, Handle, Syscall};
use arkhe_kernel_core::capability::{CapabilityTable, ObjectId, Rights};
use arkhe_kernel_core::elf::Executable;
use arkhe_kernel_core::frame_pool::FramePool;
use arkhe_kernel_core::memory::FrameRange;
use arkhe_kernel_core::scheduler::Scheduler;

use crate::arch::x86_64::{
    self as cpu,
    exceptions::ExceptionFrame,
    paging::{self, AddressSpace, POOL_FRAMES, STACK_TOP},
};

const CONSOLE: ObjectId = ObjectId(1);
const MAX_WRITE: usize = 256;
const PROCESSES: usize = 3;
const ROUNDS: u64 = 2;

struct Process {
    space: AddressSpace,
    capabilities: CapabilityTable<4>,
    context: ExceptionFrame,
    ticks: u64,
}
impl Process {
    const fn new() -> Self {
        Self {
            space: AddressSpace::new(),
            capabilities: CapabilityTable::new(),
            context: ExceptionFrame::EMPTY,
            ticks: 0,
        }
    }
}
struct State {
    processes: [Process; PROCESSES],
    frames: FramePool<POOL_FRAMES>,
    scheduler: Scheduler<PROCESSES>,
    image: Option<Executable<'static>>,
    kernel_root: u64,
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
    image: None,
    kernel_root: 0,
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

pub fn launch(ranges: &[FrameRange], direct_map: u64, elf: Executable<'static>) -> ! {
    cpu::disable_interrupts();
    assert!(
        !INITIALIZED.swap(true, Ordering::AcqRel),
        "process state already initialized"
    );
    let state = state();
    state.frames.initialize(ranges).expect("usable frame pool");
    state.kernel_root = paging::active_root();
    state.direct_map = direct_map;
    state.image = Some(elf);
    #[cfg(feature = "smoke")]
    check_load_rollback(state, ranges);
    spawn_round(state);
    let slot = state.scheduler.advance().expect("first runnable process");
    cpu::timer::init();
    cpu::write_console(format_args!(
        "scheduler: three isolated Rust processes, timer ~100 Hz\n"
    ));
    let process = &state.processes[slot];
    // SAFETY: The static frame has validated RX entry, RW/NX stack, ring-3 selectors,
    // IF=1/IOPL=0 and zeroed GPRs. All IRQ gates are installed before enabling IF.
    unsafe { arkhe_enter_user(process.space.root(), &process.context) }
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
    assert_eq!(state.frames.live_count(), 0, "previous round leaked frames");
    for (role, process) in state.processes.iter_mut().enumerate() {
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
            .insert(CONSOLE, Rights::SEND)
            .expect("console capability");
        process.context = ExceptionFrame::user(
            state.image.as_ref().expect("init image").entry(),
            STACK_TOP - 8,
            console.raw(),
            role as u64,
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
            cpu::write_console(format_args!("process {slot}: exited with status {a}\n"));
            state.failed |= a != 0;
            retire(state, frame);
        }
        Ok(Syscall::Yield) => {
            frame.result(0);
            state.yields = state.yields.saturating_add(1);
            state.processes[slot].context = *frame;
            state.scheduler.advance();
            restore(state, frame);
        }
        call => {
            let process = &mut state.processes[slot];
            let result = match call {
                Ok(Syscall::Version) => Ok(u64::from(ABI_VERSION)),
                Ok(Syscall::ClockTicks) => Ok(cpu::timer::ticks().min(i64::MAX as u64)),
                Ok(Syscall::Close) => process.capabilities.close(Handle::from_raw(a)).map(|()| 0),
                Ok(Syscall::DebugWrite) => write(process, a, b, c),
                _ => Err(Error::Unsupported),
            };
            frame.result(match result {
                Ok(value) => value as i64,
                Err(error) => error as i64,
            });
        }
    }
}

pub fn preempt(frame: &mut ExceptionFrame) {
    let state = state();
    let slot = state.scheduler.current().expect("running timer process");
    state.processes[slot].ticks = state.processes[slot].ticks.saturating_add(1);
    state.preemptions = state.preemptions.saturating_add(1);
    #[cfg(feature = "smoke")]
    if env!("ARKHE_TEST_CASE") == "scheduler" && slot == 0 && state.processes[slot].ticks == 8 {
        cpu::write_console(format_args!(
            "scheduler: stopped non-cooperative test process\n"
        ));
        retire(state, frame);
        return;
    }
    state.processes[slot].context = *frame;
    state.scheduler.advance();
    restore(state, frame);
}

fn restore(state: &State, frame: &mut ExceptionFrame) {
    let slot = state.scheduler.current().expect("selected process");
    let process = &state.processes[slot];
    *frame = process.context;
    // SAFETY: All process roots retain the same supervisor kernel/IST mappings.
    // No Rust reference points into the outgoing user mappings; IRQs remain masked.
    unsafe {
        paging::switch(process.space.root());
    }
}

fn retire(state: &mut State, frame: &mut ExceptionFrame) {
    let slot = state.scheduler.remove_current().expect("live process");
    state.completed += 1;
    let root = state
        .scheduler
        .current()
        .map_or(state.kernel_root, |next| state.processes[next].space.root());
    // SAFETY: The next process or original boot root retains all kernel/IST mappings.
    // Switching first flushes the outgoing non-global TLB entries before reclamation.
    unsafe {
        paging::switch(root);
    }
    state.processes[slot].space.release(&mut state.frames);
    state.processes[slot].capabilities = CapabilityTable::new();
    if state.scheduler.current().is_none() {
        assert_eq!(state.frames.live_count(), 0, "process frame leak");
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
    if process
        .capabilities
        .resolve(Handle::from_raw(handle), Rights::SEND)?
        != CONSOLE
    {
        return Err(Error::PermissionDenied);
    }
    let length = usize::try_from(length).map_err(|_| Error::InvalidArgument)?;
    if length > MAX_WRITE {
        return Err(Error::InvalidArgument);
    }
    let mut buffer = [0; MAX_WRITE];
    process
        .space
        .copy_from_user(pointer, &mut buffer[..length])
        .map_err(|_| Error::InvalidArgument)?;
    let text = core::str::from_utf8(&buffer[..length]).map_err(|_| Error::InvalidArgument)?;
    cpu::write_console(format_args!("{text}"));
    Ok(length as u64)
}

fn finish(state: &State) -> ! {
    assert_eq!(state.completed, PROCESSES * ROUNDS as usize);
    cpu::write_console(format_args!(
        "scheduler: all processes stopped; preemptions={}, yields={}\n",
        state.preemptions, state.yields
    ));
    #[cfg(feature = "smoke")]
    {
        let case = env!("ARKHE_TEST_CASE");
        let expected = if matches!(case, "boot" | "scheduler" | "idle") {
            0
        } else {
            ROUNDS as usize
        };
        if state.failed || state.expected_faults != expected || state.yields == 0 {
            cpu::exit_test(0x11);
        }
        if case == "idle" {
            cpu::timer::begin_idle_test();
            cpu::idle();
        }
        let marker = match case {
            "boot" => "ARKHE_USER_OK",
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
    #[cfg(not(feature = "smoke"))]
    cpu::idle();
}

pub fn fault(frame: &mut ExceptionFrame, vector: u64, error: u64, address: u64) {
    let state = state();
    let slot = state.scheduler.current().expect("faulting process");
    cpu::write_console(format_args!(
        "process {slot}: exception {vector}, error {error:#x}, address {address:#x}\n"
    ));
    let expected = slot == 0
        && matches!(
            (env!("ARKHE_TEST_CASE"), vector, error),
            ("user-kernel-read", 14, 5)
                | ("user-cli", 13, 0)
                | ("user-write-code", 14, 7)
                | ("user-exec-stack", 14, 0x15)
                | ("user-fpu", 7, 0)
        );
    if expected {
        state.expected_faults += 1;
    } else {
        state.failed = true;
    }
    retire(state, frame);
}
