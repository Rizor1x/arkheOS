use core::sync::atomic::{AtomicU64, Ordering};

static TICKS: AtomicU64 = AtomicU64::new(0);
const DIVISOR: u16 = 11_932; // 1.193182 MHz / divisor, approximately 100 Hz.

pub fn init() {
    super::disable_interrupts();
    for (port, value) in [
        (0x21, 0xff),
        (0xa1, 0xff),
        (0x20, 0x11),
        (0xa0, 0x11),
        (0x21, 32),
        (0xa1, 40),
        (0x21, 4),
        (0xa1, 2),
        (0x21, 1),
        (0xa1, 1),
    ] {
        output(port, value);
        output(0x80, 0);
    }
    output(0x43, 0x34); // Counter 0, low/high bytes, mode 2, binary.
    output(0x40, DIVISOR as u8);
    output(0x40, (DIVISOR >> 8) as u8);
    output(0x21, 0xfe);
    output(0xa1, 0xff);
}

pub fn tick() {
    let previous = TICKS.load(Ordering::Relaxed);
    TICKS.store(previous.saturating_add(1), Ordering::Relaxed);
    output(0x20, 0x20);
    #[cfg(feature = "smoke")]
    if IDLE_START.load(Ordering::Relaxed) != u64::MAX
        && ticks().saturating_sub(IDLE_START.load(Ordering::Relaxed)) >= 3
    {
        super::write_console(format_args!("ARKHE_IDLE_OK\n"));
        super::exit_test(0x10);
    }
}

pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn spurious(vector: u64) {
    let port = if vector == 39 { 0x20 } else { 0xa0 };
    output(port, 0x0b);
    if input(port) & 0x80 != 0 {
        output(port, 0x20);
    }
    if vector == 47 {
        output(0x20, 0x20);
    }
}

fn output(port: u16, value: u8) {
    // SAFETY: Ring 0 owns the legacy PIC/PIT ports on the supported PC platform.
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
    }
}

fn input(port: u16) -> u8 {
    let value: u8;
    // SAFETY: Reading the owned PIC command port returns its selected status register.
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack));
    }
    value
}

#[cfg(feature = "smoke")]
static IDLE_START: AtomicU64 = AtomicU64::new(u64::MAX);

#[cfg(feature = "smoke")]
pub fn begin_idle_test() {
    IDLE_START.store(ticks(), Ordering::Relaxed);
}
