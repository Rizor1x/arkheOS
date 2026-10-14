pub const USER_IMAGE_START: u64 = 0x10000;
pub const USER_IMAGE_END: u64 = 0x1000_0000;
const MAX_SEGMENTS: usize = 8;
const MAX_IMAGE_PAGES: u64 = 1024;
const PAGE: u64 = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElfError {
    Truncated,
    Format,
    Unsupported,
    Permissions,
    Range,
    Overlap,
    Entry,
    TooLarge,
}

#[derive(Clone, Copy)]
pub struct Segment<'a> {
    pub address: u64,
    pub memory_size: u64,
    pub data: &'a [u8],
    pub writable: bool,
    pub executable: bool,
}

impl Segment<'_> {
    const EMPTY: Self = Self {
        address: 0,
        memory_size: 0,
        data: &[],
        writable: false,
        executable: false,
    };
}

pub struct Executable<'a> {
    entry: u64,
    segments: [Segment<'a>; MAX_SEGMENTS],
    count: usize,
}

impl<'a> Executable<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.get(..7) != Some(&b"\x7fELF\x02\x01\x01"[..]) {
            return Err(ElfError::Format);
        }
        if word(bytes, 16)? != 2
            || word(bytes, 18)? != 62
            || dword(bytes, 20)? != 1
            || word(bytes, 52)? != 64
            || word(bytes, 54)? != 56
        {
            return Err(ElfError::Unsupported);
        }
        let entry = qword(bytes, 24)?;
        let offset = usize::try_from(qword(bytes, 32)?).map_err(|_| ElfError::Range)?;
        let count = usize::from(word(bytes, 56)?);
        if count == 0 || count > MAX_SEGMENTS {
            return Err(ElfError::TooLarge);
        }
        let end = offset.checked_add(count * 56).ok_or(ElfError::Truncated)?;
        let headers = bytes.get(offset..end).ok_or(ElfError::Truncated)?;
        let mut executable = Self {
            entry,
            segments: [Segment::EMPTY; MAX_SEGMENTS],
            count: 0,
        };
        let mut previous_end = USER_IMAGE_START;
        let mut pages = 0;
        for header in headers.as_chunks::<56>().0 {
            match dword(header, 0)? {
                0 | 4 | 6 => continue,
                1 => {}
                _ => return Err(ElfError::Unsupported),
            }
            let flags = dword(header, 4)?;
            if flags & !7 != 0 || flags & 4 == 0 || flags & 3 == 3 {
                return Err(ElfError::Permissions);
            }
            let file_offset = qword(header, 8)?;
            let address = qword(header, 16)?;
            let file_size = qword(header, 32)?;
            let memory_size = qword(header, 40)?;
            let alignment = qword(header, 48)?;
            if file_size > memory_size
                || file_offset % PAGE != address % PAGE
                || (alignment > 1
                    && (!alignment.is_power_of_two()
                        || file_offset % alignment != address % alignment))
            {
                return Err(ElfError::Format);
            }
            let memory_end = address.checked_add(memory_size).ok_or(ElfError::Range)?;
            if address < USER_IMAGE_START || memory_end > USER_IMAGE_END {
                return Err(ElfError::Range);
            }
            let file_end = file_offset
                .checked_add(file_size)
                .ok_or(ElfError::Truncated)?;
            let data = bytes
                .get(
                    usize::try_from(file_offset).map_err(|_| ElfError::Range)?
                        ..usize::try_from(file_end).map_err(|_| ElfError::Range)?,
                )
                .ok_or(ElfError::Truncated)?;
            if memory_size == 0 {
                continue;
            }
            let start_page = address & !(PAGE - 1);
            let end_page = memory_end.checked_add(PAGE - 1).ok_or(ElfError::Range)? & !(PAGE - 1);
            if start_page < previous_end {
                return Err(ElfError::Overlap);
            }
            previous_end = end_page;
            pages += (end_page - start_page) / PAGE;
            if pages > MAX_IMAGE_PAGES {
                return Err(ElfError::TooLarge);
            }
            executable.segments[executable.count] = Segment {
                address,
                memory_size,
                data,
                writable: flags & 2 != 0,
                executable: flags & 1 != 0,
            };
            executable.count += 1;
        }
        if !executable.segments().iter().any(|segment| {
            segment.executable
                && (segment.address..segment.address + segment.memory_size).contains(&entry)
        }) {
            return Err(ElfError::Entry);
        }
        Ok(executable)
    }

    pub fn segments(&self) -> &[Segment<'a>] {
        &self.segments[..self.count]
    }

    pub fn entry(&self) -> u64 {
        self.entry
    }
}

fn word(bytes: &[u8], offset: usize) -> Result<u16, ElfError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(ElfError::Truncated)?
            .try_into()
            .map_err(|_| ElfError::Truncated)?,
    ))
}
fn dword(bytes: &[u8], offset: usize) -> Result<u32, ElfError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(ElfError::Truncated)?
            .try_into()
            .map_err(|_| ElfError::Truncated)?,
    ))
}
fn qword(bytes: &[u8], offset: usize) -> Result<u64, ElfError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(ElfError::Truncated)?
            .try_into()
            .map_err(|_| ElfError::Truncated)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> [u8; 4097] {
        let mut bytes = [0; 4097];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x400000u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
        bytes[68..72].copy_from_slice(&5u32.to_le_bytes());
        bytes[72..80].copy_from_slice(&4096u64.to_le_bytes());
        bytes[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
        bytes[96..104].copy_from_slice(&1u64.to_le_bytes());
        bytes[104..112].copy_from_slice(&4096u64.to_le_bytes());
        bytes[112..120].copy_from_slice(&4096u64.to_le_bytes());
        bytes[4096] = 0xc3;
        bytes
    }

    #[test]
    fn validates_static_image_and_bss() {
        let bytes = fixture();
        let elf = Executable::parse(&bytes).unwrap();
        assert_eq!(elf.entry, 0x400000);
        assert_eq!(elf.segments()[0].data, &[0xc3]);
        assert_eq!(elf.segments()[0].memory_size, 4096);
    }

    #[test]
    fn rejects_wx_dynamic_kernel_addresses_and_bad_entry() {
        for (offset, value) in [(68, 7u64), (64, 3), (80, 0xffff800000000000), (24, 0)] {
            let mut bytes = fixture();
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            assert!(Executable::parse(&bytes).is_err());
        }
    }

    #[test]
    fn rejects_truncated_overflowing_and_overlapping_images() {
        let bytes = fixture();
        for size in [0, 16, 63, 119, 4096] {
            assert!(Executable::parse(&bytes[..size]).is_err());
        }
        let mut bytes = fixture();
        bytes[104..112].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Executable::parse(&bytes).is_err());
        let mut bytes = fixture();
        bytes[56..58].copy_from_slice(&2u16.to_le_bytes());
        bytes.copy_within(64..120, 120);
        assert!(matches!(Executable::parse(&bytes), Err(ElfError::Overlap)));
    }
}
