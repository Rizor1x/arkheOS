const BLOCK: usize = 512;
const MAX_FILES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveError {
    Truncated,
    InvalidHeader,
    InvalidPath,
    Unsupported,
    Duplicate,
    TooManyFiles,
    NotFound,
}

#[derive(Clone, Copy)]
struct File<'a> {
    name: &'a str,
    bytes: &'a [u8],
    executable: bool,
}

pub struct Archive<'a> {
    files: [File<'a>; MAX_FILES],
    count: usize,
}

impl<'a> Archive<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, ArchiveError> {
        let mut archive = Self {
            files: [File {
                name: "",
                bytes: &[],
                executable: false,
            }; MAX_FILES],
            count: 0,
        };
        let mut offset = 0_usize;
        loop {
            let header = bytes
                .get(offset..offset.checked_add(BLOCK).ok_or(ArchiveError::Truncated)?)
                .ok_or(ArchiveError::Truncated)?;
            if header.iter().all(|byte| *byte == 0) {
                let tail = bytes.get(offset..).ok_or(ArchiveError::Truncated)?;
                if tail.len() < 2 * BLOCK || !tail.iter().all(|byte| *byte == 0) {
                    return Err(ArchiveError::Truncated);
                }
                break;
            }
            if &header[257..263] != b"ustar\0" || &header[263..265] != b"00" {
                return Err(ArchiveError::InvalidHeader);
            }
            let checksum: usize = header
                .iter()
                .enumerate()
                .map(|(i, byte)| {
                    if (148..156).contains(&i) {
                        usize::from(b' ')
                    } else {
                        usize::from(*byte)
                    }
                })
                .sum();
            if octal(&header[148..156])? != checksum {
                return Err(ArchiveError::InvalidHeader);
            }
            if !matches!(header[156], 0 | b'0') || header[345..500].iter().any(|byte| *byte != 0) {
                return Err(ArchiveError::Unsupported);
            }
            let end = header[..100]
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(100);
            if header[end..100].iter().any(|byte| *byte != 0) {
                return Err(ArchiveError::InvalidPath);
            }
            let name =
                core::str::from_utf8(&header[..end]).map_err(|_| ArchiveError::InvalidPath)?;
            if !valid_path(name) {
                return Err(ArchiveError::InvalidPath);
            }
            let mode = octal(&header[100..108])?;
            if mode > 0o7777 {
                return Err(ArchiveError::InvalidHeader);
            }
            let size = octal(&header[124..136])?;
            let start = offset.checked_add(BLOCK).ok_or(ArchiveError::Truncated)?;
            let data_end = start.checked_add(size).ok_or(ArchiveError::Truncated)?;
            let data = bytes.get(start..data_end).ok_or(ArchiveError::Truncated)?;
            let padded = size.checked_add(BLOCK - 1).ok_or(ArchiveError::Truncated)? & !(BLOCK - 1);
            offset = start.checked_add(padded).ok_or(ArchiveError::Truncated)?;
            if offset > bytes.len() {
                return Err(ArchiveError::Truncated);
            }
            let slot = archive
                .files
                .get_mut(archive.count)
                .ok_or(ArchiveError::TooManyFiles)?;
            *slot = File {
                name,
                bytes: data,
                executable: mode & 0o111 != 0,
            };
            archive.count += 1;
        }
        archive.files[..archive.count].sort_unstable_by(|a, b| a.name.cmp(b.name));
        if archive.files[..archive.count]
            .windows(2)
            .any(|pair| pair[0].name == pair[1].name)
        {
            return Err(ArchiveError::Duplicate);
        }
        Ok(archive)
    }

    pub fn read(&self, name: &str) -> Result<&'a [u8], ArchiveError> {
        self.lookup(name).map(|file| file.bytes)
    }
    pub fn executable(&self, name: &str) -> Result<bool, ArchiveError> {
        self.lookup(name).map(|file| file.executable)
    }
    fn lookup(&self, name: &str) -> Result<&File<'a>, ArchiveError> {
        let index = self.files[..self.count]
            .binary_search_by(|file| file.name.cmp(name))
            .map_err(|_| ArchiveError::NotFound)?;
        Ok(&self.files[index])
    }

    pub fn file_count(&self) -> usize {
        self.count
    }
}

pub(crate) fn valid_path(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.bytes().any(|byte| byte < 32 || byte == 127)
        && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn octal(bytes: &[u8]) -> Result<usize, ArchiveError> {
    let mut value = 0_usize;
    let mut ended = false;
    let mut digits = false;
    for byte in bytes {
        if matches!(byte, 0 | b' ') {
            if digits {
                ended = true;
            }
        } else if (b'0'..=b'7').contains(byte) && !ended {
            digits = true;
            value = value
                .checked_mul(8)
                .and_then(|n| n.checked_add(usize::from(byte - b'0')))
                .ok_or(ArchiveError::InvalidHeader)?;
        } else {
            return Err(ArchiveError::InvalidHeader);
        }
    }
    if !digits {
        return Err(ArchiveError::InvalidHeader);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{format, vec, vec::Vec};

    fn fixture(name: &str, body: &[u8]) -> Vec<u8> {
        let mut data = vec![0; BLOCK + body.len().div_ceil(BLOCK) * BLOCK + 2 * BLOCK];
        data[..name.len()].copy_from_slice(name.as_bytes());
        data[100..108].copy_from_slice(b"0000444\0");
        data[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
        data[156] = b'0';
        data[257..263].copy_from_slice(b"ustar\0");
        data[263..265].copy_from_slice(b"00");
        fix_checksum(&mut data);
        data[BLOCK..BLOCK + body.len()].copy_from_slice(body);
        data
    }

    fn fix_checksum(data: &mut [u8]) {
        data[148..156].fill(b' ');
        let sum: usize = data[..BLOCK].iter().map(|b| usize::from(*b)).sum();
        data[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    }

    #[test]
    fn reads_valid_archive_and_handles_missing_files() {
        let data = fixture("etc/motd", b"hello");
        let archive = Archive::new(&data).unwrap();
        assert_eq!(archive.file_count(), 1);
        assert_eq!(archive.read("etc/motd"), Ok(&b"hello"[..]));
        assert_eq!(archive.read("missing"), Err(ArchiveError::NotFound));
        assert!(Archive::new(&[0; 1024]).is_ok());
    }

    #[test]
    fn rejects_truncation_bad_checksum_and_invalid_size() {
        let mut data = fixture("a", b"hello");
        for length in [0, 100, 512, 516, 1024, data.len() - 1] {
            assert!(Archive::new(&data[..length]).is_err());
        }
        data[0] ^= 1;
        assert!(matches!(
            Archive::new(&data),
            Err(ArchiveError::InvalidHeader)
        ));
        data[124..136].fill(b'7');
        fix_checksum(&mut data);
        assert!(Archive::new(&data).is_err());
    }

    #[test]
    fn rejects_path_traversal_links_and_duplicates() {
        for name in ["/root", "../root", "a/../b", "a//b", "a/./b", "a\\b"] {
            assert!(matches!(
                Archive::new(&fixture(name, b"")),
                Err(ArchiveError::InvalidPath)
            ));
        }
        let mut data = fixture("a", b"");
        data[156] = b'2';
        fix_checksum(&mut data);
        assert!(matches!(
            Archive::new(&data),
            Err(ArchiveError::Unsupported)
        ));
        let first = fixture("a", b"first");
        let second = fixture("a", b"second");
        let mut combined = first[..1024].to_vec();
        combined.extend_from_slice(&second);
        assert!(matches!(
            Archive::new(&combined),
            Err(ArchiveError::Duplicate)
        ));
    }

    #[test]
    fn checks_execute_mode_and_rejects_malformed_permissions() {
        let mut data = fixture("bin/app", b"code");
        assert_eq!(
            Archive::new(&data).unwrap().executable("bin/app"),
            Ok(false)
        );
        data[100..108].copy_from_slice(b"0000555\0");
        fix_checksum(&mut data);
        assert_eq!(Archive::new(&data).unwrap().executable("bin/app"), Ok(true));
        data[100..108].copy_from_slice(b"0077777\0");
        fix_checksum(&mut data);
        assert!(matches!(
            Archive::new(&data),
            Err(ArchiveError::InvalidHeader)
        ));
        data[100] = b'9';
        fix_checksum(&mut data);
        assert!(matches!(
            Archive::new(&data),
            Err(ArchiveError::InvalidHeader)
        ));
    }

    #[test]
    fn accepts_unaligned_input_without_pointer_casts() {
        let mut data = vec![42];
        data.extend_from_slice(&fixture("a", b"b"));
        assert_eq!(Archive::new(&data[1..]).unwrap().read("a"), Ok(&b"b"[..]));
    }
}
