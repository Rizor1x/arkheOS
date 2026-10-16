#![no_std]

#[path = "../kernel/src/boot.rs"]
mod boot;
#[path = "../kernel/src/arch/x86_64/paging.rs"]
mod paging;

use arkhe_kernel_core::{elf::Executable, frame_pool::FramePool};
use paging::{KernelSpace, UserSpace};

pub fn valid_roles(
    kernel: &mut KernelSpace,
    user: &mut UserSpace,
    frames: &mut FramePool<4096>,
    image: &Executable<'_>,
) {
    // SAFETY: This fixture is type-checked only; it is never linked or executed.
    let _ = unsafe { user.load(frames, kernel, image) };
    let _ = user.copy_to_user(0x10000, b"x");
    let _ = user.release(frames, kernel);
}

#[cfg(paging_role = "kernel_release")]
pub fn misuse(kernel: &mut KernelSpace, frames: &mut FramePool<4096>) {
    let _ = kernel.release(frames);
}

#[cfg(paging_role = "kernel_load")]
pub fn misuse(kernel: &mut KernelSpace, frames: &mut FramePool<4096>, image: &Executable<'_>) {
    let _ = kernel.load(frames, image);
}

#[cfg(paging_role = "user_build")]
pub fn misuse(user: &mut UserSpace, frames: &mut FramePool<4096>, boot: &boot::BootMemory) {
    let _ = user.build_kernel(frames, boot);
}

#[cfg(paging_role = "private_tables")]
pub fn misuse(kernel: &KernelSpace) {
    let _ = &kernel.tables;
}

#[cfg(paging_role = "wrong_owner_type")]
pub fn misuse(
    user: &mut UserSpace,
    other: &mut UserSpace,
    frames: &mut FramePool<4096>,
    image: &Executable<'_>,
) {
    // SAFETY: This negative fixture must fail type checking and is never executed.
    let _ = unsafe { user.load(frames, other, image) };
}
