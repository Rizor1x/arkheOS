use super::{ADDRESS, AddressSpace, HUGE, MapError, NX, PAGE, PRESENT, USER, WRITE};
use crate::boot::BootMemory;
use arkhe_kernel_core::frame_pool::FramePool;
use arkhe_kernel_core::mapping::{Mapping, MappingPlan};

unsafe extern "C" {
    static __kernel_start: u8;
    static __text_start: u8;
    static __rodata_start: u8;
    static __data_start: u8;
    static __kernel_end: u8;
    static arkhe_boot_guard: u8;
}

const PLAN_ENTRIES: usize = crate::boot::MAX_REGIONS + 6;

impl AddressSpace {
    /// Construct an inactive supervisor root from an explicit boot mapping plan.
    /// # Safety
    /// The BSP has IRQs masked, owns `frames`, and still runs on Limine mappings.
    /// Linker regions, executable bases and module bytes must describe retained RAM.
    pub unsafe fn build_kernel<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        boot: &BootMemory,
    ) -> Result<(), MapError> {
        assert_eq!(self.count, 0, "kernel space already owns frames");
        let plan = boot_plan(boot)?;
        self.direct_map = boot.direct_map_offset;
        super::enable_protection();
        let result = self.build_plan(frames, &plan);
        if result.is_err() {
            self.release_frames(frames);
        }
        result
    }

    fn build_plan<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        plan: &MappingPlan<PLAN_ENTRIES>,
    ) -> Result<(), MapError> {
        let control: u64;
        // SAFETY: Ring 0 reads CR4; five-level paging is unsupported by this walker.
        unsafe {
            core::arch::asm!("mov {}, cr4", out(reg) control, options(nomem, nostack));
        }
        if control & ((1 << 12) | (1 << 17)) != 0 {
            return Err(MapError::InvalidAddress);
        }
        self.root = self.allocate(frames)?;
        self.zero(self.root)?;
        for mapping in plan.entries() {
            for offset in (0..mapping.length()).step_by(PAGE as usize) {
                self.map_supervisor(frames, mapping, offset)?;
            }
        }
        self.check_plan(plan)?;
        Ok(())
    }

    fn map_supervisor<const N: usize>(
        &mut self,
        frames: &mut FramePool<N>,
        mapping: Mapping,
        offset: u64,
    ) -> Result<(), MapError> {
        let address = mapping.virtual_start() + offset;
        let physical = mapping.physical_start() + offset;
        let mut table = self.root;
        for shift in [39, 30, 21] {
            let index = ((address >> shift) & 511) as usize;
            let mut entry = self.read_entry(table, index)?;
            if entry & PRESENT == 0 {
                let next = self.allocate(frames)?;
                self.zero(next)?;
                entry = next | PRESENT | WRITE;
                self.write_entry(table, index, entry)?;
            }
            if entry & (HUGE | USER) != 0 {
                return Err(MapError::InvalidAddress);
            }
            table = entry & ADDRESS;
        }
        let index = ((address >> 12) & 511) as usize;
        if self.read_entry(table, index)? != 0 {
            return Err(MapError::AlreadyMapped);
        }
        let flags = PRESENT
            | if mapping.writable() { WRITE } else { 0 }
            | if mapping.executable() { 0 } else { NX };
        self.write_entry(table, index, physical | flags)
    }

    fn check_plan(&self, plan: &MappingPlan<PLAN_ENTRIES>) -> Result<(), MapError> {
        for mapping in plan.entries() {
            for offset in (0..mapping.length()).step_by(PAGE as usize) {
                let leaf = self.supervisor_leaf(mapping.virtual_start() + offset)?;
                if leaf & ADDRESS != mapping.physical_start() + offset
                    || (leaf & WRITE != 0) != mapping.writable()
                    || (leaf & NX == 0) != mapping.executable()
                {
                    return Err(MapError::InvalidAddress);
                }
            }
        }
        for index in 0..256 {
            if self.read_entry(self.root, index)? != 0 {
                return Err(MapError::InvalidAddress);
            }
        }
        Ok(())
    }

    pub fn table_frames(&self) -> usize {
        self.count
    }

    pub fn supervisor_leaf(&self, address: u64) -> Result<u64, MapError> {
        self.supervisor_slot(address).map(|(_, _, entry)| entry)
    }

    fn supervisor_slot(&self, address: u64) -> Result<(u64, usize, u64), MapError> {
        let mut table = self.root;
        for shift in [39, 30, 21, 12] {
            let index = ((address >> shift) & 511) as usize;
            let entry = self.read_entry(table, index)?;
            if entry & PRESENT == 0 || entry & USER != 0 || (shift != 12 && entry & HUGE != 0) {
                return Err(MapError::InvalidAddress);
            }
            if shift == 12 {
                return Ok((table, index, entry));
            }
            let next = entry & ADDRESS;
            if !self.owned[..self.count]
                .iter()
                .flatten()
                .any(|owned| owned.frame.physical() == next)
            {
                return Err(MapError::InvalidAddress);
            }
            table = next;
        }
        Err(MapError::InvalidAddress)
    }

    pub(super) fn set_alias_write(
        &mut self,
        frame: &arkhe_kernel_core::frame_pool::Frame,
        writable: bool,
    ) -> Result<(), MapError> {
        let physical = frame.physical();
        let address = self.pointer(physical)? as u64;
        let (table, index, entry) = self.supervisor_slot(address)?;
        if entry & ADDRESS != physical || entry & NX == 0 || (entry & WRITE != 0) == writable {
            return Err(MapError::InvalidAddress);
        }
        let entry = if writable {
            entry | WRITE
        } else {
            entry & !WRITE
        };
        self.write_entry(table, index, entry)?;
        // SAFETY: Only this IRQ-masked BSP changes the shared supervisor leaf.
        // PCID/PGE are disabled: INVLPG covers this root; CR3 flushes inactive roots.
        // The asm memory clobber keeps the PTE store before invalidation.
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags));
        }
        Ok(())
    }
}

fn boot_plan(boot: &BootMemory) -> Result<MappingPlan<PLAN_ENTRIES>, MapError> {
    let [start, text, rodata, data, end, guard] = [
        core::ptr::addr_of!(__kernel_start) as u64,
        core::ptr::addr_of!(__text_start) as u64,
        core::ptr::addr_of!(__rodata_start) as u64,
        core::ptr::addr_of!(__data_start) as u64,
        core::ptr::addr_of!(__kernel_end) as u64,
        core::ptr::addr_of!(arkhe_boot_guard) as u64,
    ];
    let guard_end = guard.checked_add(PAGE).ok_or(MapError::InvalidAddress)?;
    let kernel_length = end.checked_sub(start).ok_or(MapError::InvalidAddress)?;
    let kernel_physical_end = boot
        .physical_base
        .checked_add(kernel_length)
        .ok_or(MapError::InvalidAddress)?;
    if boot.virtual_base != start || !(data <= guard && guard_end < end) {
        return Err(MapError::InvalidAddress);
    }
    let mut plan = MappingPlan::new();
    for (begin, finish, writable, executable) in [
        (start, text, true, false),
        (text, rodata, false, true),
        (rodata, data, false, false),
        (data, guard, true, false),
        (guard_end, end, true, false),
    ] {
        if begin == finish {
            continue;
        }
        let offset = begin.checked_sub(start).ok_or(MapError::InvalidAddress)?;
        let physical = boot
            .physical_base
            .checked_add(offset)
            .ok_or(MapError::InvalidAddress)?;
        insert(
            &mut plan,
            begin,
            physical,
            finish.checked_sub(begin).ok_or(MapError::InvalidAddress)?,
            writable,
            executable,
        )?;
    }
    let physical = boot
        .module_address
        .checked_sub(boot.direct_map_offset)
        .ok_or(MapError::InvalidAddress)?;
    let module_end = physical
        .checked_add(boot.module_size)
        .and_then(|end| end.checked_add(PAGE - 1))
        .ok_or(MapError::InvalidAddress)?
        & !(PAGE - 1);
    let module_start = physical & !(PAGE - 1);
    // The module is the sole read-only boot allocation retained in the direct map.
    insert(
        &mut plan,
        boot.direct_map_offset
            .checked_add(module_start)
            .ok_or(MapError::InvalidAddress)?,
        module_start,
        module_end
            .checked_sub(module_start)
            .ok_or(MapError::InvalidAddress)?,
        false,
        false,
    )?;
    for range in boot.ranges() {
        if (range.start() < kernel_physical_end && boot.physical_base < range.end())
            || (range.start() < module_end && module_start < range.end())
        {
            return Err(MapError::InvalidAddress);
        }
        insert(
            &mut plan,
            boot.direct_map_offset
                .checked_add(range.start())
                .ok_or(MapError::InvalidAddress)?,
            range.start(),
            range.end() - range.start(),
            true,
            false,
        )?;
    }
    Ok(plan)
}

fn insert(
    plan: &mut MappingPlan<PLAN_ENTRIES>,
    virtual_start: u64,
    physical_start: u64,
    length: u64,
    writable: bool,
    executable: bool,
) -> Result<(), MapError> {
    let mapping = Mapping::new(virtual_start, physical_start, length, writable, executable)
        .map_err(|_| MapError::InvalidAddress)?;
    plan.insert(mapping).map_err(|_| MapError::InvalidAddress)
}

#[cfg(feature = "smoke")]
pub fn check_rollback(boot: &BootMemory) {
    let mut frames = FramePool::<4>::new();
    frames
        .initialize(boot.ranges())
        .expect("kernel rollback pool");
    let mut space = AddressSpace::new();
    // SAFETY: Before the main pool issues any frames, this temporary pool alone
    // owns usable RAM. build_kernel releases every lease on its forced failure.
    let result = unsafe { space.build_kernel(&mut frames, boot) };
    assert_eq!(result, Err(MapError::NoMemory));
    assert_eq!(
        frames.live_count(),
        0,
        "kernel table rollback leaked frames"
    );
}

#[cfg(feature = "smoke")]
static NX_PROBE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0xc3);

#[cfg(feature = "smoke")]
fn fault_target(boot: &BootMemory) -> Option<(u64, u64)> {
    let text = core::ptr::addr_of!(__text_start) as u64;
    match env!("ARKHE_TEST_CASE") {
        "kernel-write-code" => Some((text, 3)),
        "kernel-write-alias" => {
            let physical = boot
                .physical_base
                .checked_add(text.checked_sub(boot.virtual_base)?)?;
            Some((boot.direct_map_offset.checked_add(physical)?, 2))
        }
        "kernel-exec-data" => Some((NX_PROBE.as_ptr() as u64, 17)),
        "kernel-stack-guard" => Some((core::ptr::addr_of!(arkhe_boot_guard) as u64, 0)),
        "kernel-write-module" => Some((boot.module_address, 3)),
        _ => None,
    }
}

#[cfg(feature = "smoke")]
static EXPECTED_ADDRESS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "smoke")]
static EXPECTED_ERROR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[cfg(feature = "smoke")]
pub fn expected_fault(vector: u64, error: u64, address: u64) -> bool {
    use core::sync::atomic::Ordering;
    let expected = EXPECTED_ADDRESS.load(Ordering::Relaxed);
    expected != 0
        && vector == 14
        && address == expected
        && error == EXPECTED_ERROR.load(Ordering::Relaxed)
}

#[cfg(feature = "smoke")]
pub fn trigger_alias_fault(address: u64) -> ! {
    use core::sync::atomic::Ordering;
    EXPECTED_ADDRESS.store(address, Ordering::Relaxed);
    EXPECTED_ERROR.store(3, Ordering::Relaxed);
    // SAFETY: The target is a retained user code frame's supervisor RO/NX alias.
    // IRQs are masked and the installed IST handler must observe a write-protection fault.
    unsafe {
        core::arch::asm!("mov byte ptr [{address}], 0", address = in(reg) address);
    }
    panic!("user code direct map remained writable");
}

#[cfg(feature = "smoke")]
pub fn trigger_fault(boot: &BootMemory) {
    use core::sync::atomic::Ordering;
    let Some((address, error)) = fault_target(boot) else {
        return;
    };
    EXPECTED_ADDRESS.store(address, Ordering::Relaxed);
    EXPECTED_ERROR.store(error, Ordering::Relaxed);
    // SAFETY: Fault injection uses raw addresses under the audited supervisor root,
    // with IRQs off and an installed IST handler. The operation must fault before
    // changing protected bytes or executing data; that handler terminates QEMU.
    unsafe {
        match env!("ARKHE_TEST_CASE") {
            "kernel-exec-data" => {
                core::arch::asm!("call {address}", address = in(reg) address, clobber_abi("C"))
            }
            "kernel-stack-guard" => {
                core::arch::asm!("mov rax, [{address}]", address = in(reg) address, out("rax") _)
            }
            _ => core::arch::asm!("mov byte ptr [{address}], 0", address = in(reg) address),
        }
    }
    panic!("kernel mapping protection did not fault");
}
