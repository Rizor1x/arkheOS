use core::fmt::{self, Write};

const COM1: u16 = 0x3f8;
const POLL_LIMIT: usize = 100_000;

pub fn init_console() {
    out(COM1 + 1, 0);
    out(COM1 + 3, 0x80);
    out(COM1, 1);
    out(COM1 + 1, 0);
    out(COM1 + 3, 3);
    out(COM1 + 2, 0xc7);
    out(COM1 + 4, 0x0b);
}

pub fn write_console(args: fmt::Arguments<'_>) {
    // Bootstrap has one CPU and interrupts disabled, so writes cannot interleave.
    let _ = Serial.write_fmt(args);
}

struct Serial;

impl Write for Serial {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                write_byte(b'\r')?;
            }
            write_byte(byte)?;
        }
        Ok(())
    }
}

fn write_byte(byte: u8) -> fmt::Result {
    for _ in 0..POLL_LIMIT {
        if input(COM1 + 5) & 0x20 != 0 {
            out(COM1, byte);
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(fmt::Error)
}

fn out(port: u16, byte: u8) {
    // SAFETY: Callers use only the PC UART's assigned COM1 ports at ring 0.
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") byte, options(nomem, nostack))
    };
}

fn input(port: u16) -> u8 {
    let byte: u8;
    // SAFETY: Callers read only COM1 status at ring 0.
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") byte, options(nomem, nostack))
    };
    byte
}
