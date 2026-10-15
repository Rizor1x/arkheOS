use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(feature = "smoke"))]
use super::halt;
use super::{disable_interrupts, write_console};

core::arch::global_asm!(include_str!("exceptions.S"));

const STACK_BYTES: usize = 16 * 1024;
const KERNEL_CODE: u16 = 8;
#[cfg(feature = "smoke")]
const DOUBLE_FAULT_STACK: usize = 1;
#[cfg(feature = "smoke")]
const TEST_FAULT_ADDRESS: u64 = 0x4000_0000_0000;

#[repr(C, packed)]
struct TablePointer {
    limit: u16,
    base: u64,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Gate {
    offset_low: u16,
    selector: u16,
    stack: u8,
    attributes: u8,
    offset_middle: u16,
    offset_high: u32,
    reserved: u32,
}

impl Gate {
    const EMPTY: Self = Self {
        offset_low: 0,
        selector: 0,
        stack: 0,
        attributes: 0,
        offset_middle: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn new(address: usize, stack: u8) -> Self {
        Self {
            offset_low: address as u16,
            selector: KERNEL_CODE,
            stack,
            attributes: 0x8e,
            offset_middle: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, packed)]
struct TaskState {
    reserved0: u32,
    privilege_stacks: [u64; 3],
    reserved1: u64,
    interrupt_stacks: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    io_map_base: u16,
}

impl TaskState {
    const EMPTY: Self = Self {
        reserved0: 0,
        privilege_stacks: [0; 3],
        reserved1: 0,
        interrupt_stacks: [0; 7],
        reserved2: 0,
        reserved3: 0,
        io_map_base: core::mem::size_of::<Self>() as u16,
    };
}

#[repr(C, align(16))]
struct Descriptors {
    gdt: [u64; 7],
    task: TaskState,
    idt: [Gate; 256],
    stacks: [[u8; STACK_BYTES]; 6],
}

struct DescriptorStorage(UnsafeCell<Descriptors>);

// SAFETY: Only the bootstrap CPU initializes this storage, once with IRQs off.
// After loading it, only the CPU writes TSS/GDT state and dedicated interrupt stacks.
unsafe impl Sync for DescriptorStorage {}

static STORAGE: DescriptorStorage = DescriptorStorage(UnsafeCell::new(Descriptors {
    gdt: [0; 7],
    task: TaskState::EMPTY,
    idt: [Gate::EMPTY; 256],
    stacks: [[0; STACK_BYTES]; 6],
}));
static INITIALIZED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    static arkhe_exception_entries: [usize; 32];
    fn arkhe_unexpected_entry();
    fn arkhe_exception_128();
    fn arkhe_exception_32();
    fn arkhe_exception_39();
    fn arkhe_exception_47();
}

const _: () = assert!(core::mem::size_of::<Gate>() == 16);
const _: () = assert!(core::mem::size_of::<TaskState>() == 104);

pub fn init() {
    disable_interrupts();
    assert!(
        !INITIALIZED.swap(true, Ordering::AcqRel),
        "descriptor tables already loaded"
    );
    // SAFETY: Initialization is exclusive, IRQs are off, no APs are running, and
    // the storage has a fixed address and remains allocated for the CPU's lifetime.
    let tables = unsafe { &mut *STORAGE.0.get() };
    tables.gdt[..5].copy_from_slice(&[
        0,
        0x00af_9b00_0000_ffff,
        0x00cf_9300_0000_ffff,
        0x00cf_f300_0000_ffff,
        0x00af_fb00_0000_ffff,
    ]);
    let mut interrupt_stacks = [0; 7];
    for (index, stack) in tables.stacks.iter().enumerate() {
        interrupt_stacks[index] = stack.as_ptr() as u64 + STACK_BYTES as u64;
    }
    tables.task = TaskState {
        privilege_stacks: [tables.stacks[4].as_ptr() as u64 + STACK_BYTES as u64, 0, 0],
        interrupt_stacks,
        ..TaskState::EMPTY
    };
    let task_address = core::ptr::addr_of!(tables.task) as u64;
    let task_limit = core::mem::size_of::<TaskState>() as u64 - 1;
    tables.gdt[5] = task_limit
        | ((task_address & 0xffff) << 16)
        | (((task_address >> 16) & 0xff) << 32)
        | (0x89 << 40)
        | (((task_address >> 24) & 0xff) << 56);
    tables.gdt[6] = task_address >> 32;
    tables
        .idt
        .fill(Gate::new(arkhe_unexpected_entry as *const () as usize, 1));
    // SAFETY: The assembly table defines exactly 32 immutable, valid entry addresses.
    for (vector, address) in unsafe { arkhe_exception_entries }.iter().enumerate() {
        let stack = match vector {
            8 => 2,
            2 => 3,
            18 => 4,
            _ => 1,
        };
        tables.idt[vector] = Gate::new(*address, stack);
    }
    for (vector, address) in [
        (32, arkhe_exception_32 as *const () as usize),
        (39, arkhe_exception_39 as *const () as usize),
        (47, arkhe_exception_47 as *const () as usize),
    ] {
        tables.idt[vector] = Gate::new(address, 6);
    }
    tables.idt[128] = Gate::new(arkhe_exception_128 as *const () as usize, 5);
    tables.idt[128].attributes = 0xee;
    let gdt = TablePointer {
        limit: (core::mem::size_of_val(&tables.gdt) - 1) as u16,
        base: tables.gdt.as_ptr() as u64,
    };
    let idt = TablePointer {
        limit: (core::mem::size_of_val(&tables.idt) - 1) as u16,
        base: tables.idt.as_ptr() as u64,
    };
    // SAFETY: Both descriptors point to initialized static storage. The selectors
    // match this GDT, TSS is available, and all IDT entries use valid IST stacks.
    unsafe {
        core::arch::asm!(
            "lgdt [{gdt}]", "push 8", "lea rax, [rip + 2f]", "push rax", "retfq", "2:",
            "mov ax, 16", "mov ds, ax", "mov es, ax", "mov ss, ax",
            "xor eax, eax", "mov fs, ax", "mov gs, ax",
            "mov ax, 40", "ltr ax", "lidt [{idt}]",
            gdt = in(reg) &gdt, idt = in(reg) &idt, out("rax") _,
        );
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct ExceptionFrame {
    registers: [u64; 15],
    vector: u64,
    error: u64,
    instruction: u64,
    code_segment: u64,
    flags: u64,
    stack_pointer: u64,
    stack_segment: u64,
}

const _: () = assert!(core::mem::size_of::<ExceptionFrame>() == 22 * 8);

impl ExceptionFrame {
    pub const EMPTY: Self = Self {
        registers: [0; 15],
        vector: 0,
        error: 0,
        instruction: 0,
        code_segment: 35,
        flags: 0x202,
        stack_pointer: 0,
        stack_segment: 27,
    };

    pub fn user(entry: u64, stack: u64, console: u64, role: u64, root: u64) -> Self {
        let mut frame = Self {
            instruction: entry,
            stack_pointer: stack,
            ..Self::EMPTY
        };
        frame.registers[8] = console;
        frame.registers[9] = role;
        frame.registers[12] = root;
        frame
    }

    pub fn arguments(&self) -> [u64; 4] {
        [
            self.registers[14],
            self.registers[8],
            self.registers[9],
            self.registers[12],
        ]
    }

    pub fn result(&mut self, value: i64) {
        self.registers[14] = value as u64;
    }
    pub fn is_user(&self) -> bool {
        self.code_segment & 3 == 3
    }
}

#[unsafe(no_mangle)]
extern "C" fn arkhe_exception_dispatch(frame: &mut ExceptionFrame) {
    if frame.vector == 32 {
        super::timer::tick();
        if frame.is_user() {
            crate::user::preempt(frame);
        }
        return;
    }
    if matches!(frame.vector, 39 | 47) {
        super::timer::spurious(frame.vector);
        return;
    }
    if frame.vector == 128 && frame.is_user() {
        crate::user::syscall(frame);
        return;
    }
    if frame.vector == 3 {
        return;
    }
    let fault_address: u64;
    // SAFETY: The handler runs at ring 0; reading CR2 does not modify CPU state.
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) fault_address, options(nomem, nostack));
    }
    if frame.is_user() && !matches!(frame.vector, 2 | 8 | 18) {
        crate::user::fault(frame, frame.vector, frame.error, fault_address);
        return;
    }
    write_console(format_args!(
        "exception: vector={} error={:#x} rip={:#x} cr2={fault_address:#x}\n",
        frame.vector, frame.error, frame.instruction
    ));
    #[cfg(feature = "smoke")]
    if expected_fault(frame, fault_address) {
        let marker = match frame.vector {
            0 => "ARKHE_DIVIDE_OK",
            14 => "ARKHE_PAGE_FAULT_OK",
            8 => "ARKHE_DOUBLE_FAULT_OK",
            _ => "INVALID",
        };
        write_console(format_args!("{marker}\n"));
        super::exit_test(0x10);
    }
    #[cfg(feature = "smoke")]
    super::exit_test(0x11);
    #[cfg(not(feature = "smoke"))]
    halt();
}

pub fn check_breakpoint() {
    let restored: u64;
    // SAFETY: INT3 uses our installed IST gate; the returning handler restores all
    // GPRs and RFLAGS. No maskable interrupts are enabled by this test.
    unsafe {
        core::arch::asm!(
            "mov rax, 0x13579", "mov rcx, 0x2468", "stc", "int3", "setc dl",
            "cmp rax, 0x13579", "sete al", "cmp rcx, 0x2468", "sete cl",
            "and al, cl", "and al, dl",
            out("rax") restored, out("rcx") _, out("rdx") _,
        );
    }
    assert_eq!(
        restored & 0xff,
        1,
        "breakpoint register/flags restoration failed"
    );
}

#[cfg(feature = "smoke")]
fn expected_fault(frame: &ExceptionFrame, fault_address: u64) -> bool {
    match env!("ARKHE_TEST_CASE") {
        "divide" => frame.vector == 0 && frame.error == 0,
        "page-fault" => {
            frame.vector == 14 && frame.error == 0 && fault_address == TEST_FAULT_ADDRESS
        }
        "double-fault" => {
            // SAFETY: We form addresses only, without borrowing CPU-owned stack memory.
            let start = unsafe {
                core::ptr::addr_of!((*STORAGE.0.get()).stacks[DOUBLE_FAULT_STACK]) as u64
            };
            let current = frame as *const ExceptionFrame as u64;
            frame.vector == 8
                && frame.error == 0
                && (start..start + STACK_BYTES as u64).contains(&current)
        }
        _ => false,
    }
}

#[cfg(feature = "smoke")]
pub fn trigger_test_fault() {
    match env!("ARKHE_TEST_CASE") {
        "divide" => {
            // SAFETY: This test intentionally raises #DE; the installed handler exits QEMU.
            unsafe {
                core::arch::asm!("mov eax, 1", "xor edx, edx", "xor ecx, ecx", "div ecx",
                out("rax") _, out("rdx") _, out("rcx") _);
            }
        }
        "double-fault" | "page-fault" => {
            if env!("ARKHE_TEST_CASE") == "double-fault" {
                // SAFETY: Single-CPU fault injection with IRQs off. Invalidating #PF's
                // code selector forces #DF during delivery; its independent IST stays valid.
                unsafe {
                    (*STORAGE.0.get()).idt[14].selector = 0;
                }
            }
            // SAFETY: The assembly deliberately accesses an unmapped canonical address
            // to raise #PF. It does not construct a Rust reference to invalid memory.
            unsafe {
                core::arch::asm!("mov rax, [{address}]", address = in(reg) TEST_FAULT_ADDRESS, out("rax") _);
            }
        }
        "idle" | "scheduler" | "user-fpu" | "boot" | "user-kernel-read" | "user-cli"
        | "user-write-code" | "user-exec-stack" => return,
        _ => panic!("unknown fault test"),
    }
    panic!("expected CPU exception was not raised");
}
