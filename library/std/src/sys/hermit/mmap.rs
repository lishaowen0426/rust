use crate::fs::File;
use crate::io::{Error, ErrorKind};

#[allow(dead_code)]
#[stable(feature = "rust1", since = "1.0.0")]
pub unsafe fn mmap(_f: &File, _ptr: *mut *mut u8) -> crate::io::Result<usize> {
    Err(Error::new(ErrorKind::Other, "mmap not implemented"))
}
