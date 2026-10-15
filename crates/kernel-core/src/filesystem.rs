use arkhe_abi::{Error, MAX_IO_BYTES, MAX_PATH_BYTES};

use crate::initramfs::{Archive, ArchiveError, valid_path};

/// Immutable boot namespace. File data borrows the retained module, not a RAM copy.
pub struct ReadOnlyFs<'a> {
    archive: Archive<'a>,
}

impl<'a> ReadOnlyFs<'a> {
    pub const fn new(archive: Archive<'a>) -> Self {
        Self { archive }
    }

    pub fn open(&self, path: &str) -> Result<OpenFile<'a>, Error> {
        if path.len() > MAX_PATH_BYTES || !valid_path(path) {
            return Err(Error::InvalidArgument);
        }
        let bytes = self.archive.read(path).map_err(|error| match error {
            ArchiveError::NotFound => Error::NotFound,
            _ => Error::InvalidArgument,
        })?;
        Ok(OpenFile { bytes, position: 0 })
    }
}

#[derive(Clone, Copy)]
pub struct OpenFile<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl OpenFile<'_> {
    /// Advance only after the whole bounded chunk has reached its destination.
    /// At EOF or for length zero, the callback receives an empty slice.
    pub fn read_with(
        &mut self,
        length: usize,
        copy: impl FnOnce(&[u8]) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        if length > MAX_IO_BYTES {
            return Err(Error::InvalidArgument);
        }
        let length = length.min(self.bytes.len() - self.position);
        let end = self.position + length;
        copy(&self.bytes[self.position..end])?;
        self.position = end;
        Ok(length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_reads_eof_and_zero_length_do_not_skip_data() {
        let mut file = OpenFile {
            bytes: b"hello",
            position: 0,
        };
        assert_eq!(
            file.read_with(0, |data| {
                assert!(data.is_empty());
                Ok(())
            }),
            Ok(0)
        );
        assert_eq!(
            file.read_with(2, |data| {
                assert_eq!(data, b"he");
                Ok(())
            }),
            Ok(2)
        );
        let mut independent = OpenFile {
            bytes: file.bytes,
            position: 0,
        };
        independent
            .read_with(1, |data| {
                assert_eq!(data, b"h");
                Ok(())
            })
            .unwrap();
        assert_eq!(
            file.read_with(MAX_IO_BYTES, |data| {
                assert_eq!(data, b"llo");
                Ok(())
            }),
            Ok(3)
        );
        assert_eq!(
            file.read_with(8, |data| {
                assert!(data.is_empty());
                Ok(())
            }),
            Ok(0)
        );
    }

    #[test]
    fn failure_and_oversized_reads_preserve_offset() {
        let mut file = OpenFile {
            bytes: b"abc",
            position: 0,
        };
        assert_eq!(
            file.read_with(2, |_| Err(Error::InvalidArgument)),
            Err(Error::InvalidArgument)
        );
        assert_eq!(
            file.read_with(usize::MAX, |_| panic!("copy must not run")),
            Err(Error::InvalidArgument)
        );
        assert_eq!(
            file.read_with(3, |data| {
                assert_eq!(data, b"abc");
                Ok(())
            }),
            Ok(3)
        );
    }

    #[test]
    fn rejects_noncanonical_paths_and_absent_files() {
        let fs = ReadOnlyFs::new(Archive::new(&[0; 1024]).unwrap());
        for path in [
            "",
            "/etc/motd",
            "../etc/motd",
            "etc/../motd",
            "etc//motd",
            "./etc/motd",
            "etc\\motd",
            "etc/\0motd",
            "etc/\nmotd",
        ] {
            assert!(matches!(fs.open(path), Err(Error::InvalidArgument)));
        }
        let too_long = [b'a'; MAX_PATH_BYTES + 1];
        assert!(matches!(
            fs.open(core::str::from_utf8(&too_long).unwrap()),
            Err(Error::InvalidArgument)
        ));
        assert!(matches!(fs.open("etc/motd"), Err(Error::NotFound)));
    }
}
