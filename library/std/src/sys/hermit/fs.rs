use crate::ffi::{CStr, OsString};
use crate::fmt;
use crate::io::{self, Error, ErrorKind};
use crate::io::{BorrowedCursor, IoSlice, IoSliceMut, SeekFrom};
use crate::mem;
use crate::os::hermit::io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};
use crate::path::{Path, PathBuf};
use crate::sys::common::small_c_string::run_path_with_cstr;
use crate::sys::cvt;
use crate::sys::hermit::abi::{
    self, O_APPEND, O_CREAT, O_EXCL, O_RDONLY, O_RDWR, O_TRUNC, O_WRONLY, S_IFDIR, S_IFLNK, S_IFMT,
    S_IFREG,
};
use crate::sys::hermit::fd::FileDesc;
use crate::sys::time::SystemTime;
use crate::sys::unsupported;
use crate::sys_common::{AsInner, AsInnerMut, FromInner, IntoInner};
use alloc::vec::IntoIter;

pub use crate::sys_common::fs::{copy, try_exists};
//pub use crate::sys_common::fs::remove_dir_all;

#[derive(Debug)]
pub struct File(FileDesc);

#[derive(Copy, Clone, Debug, Default)]
pub struct FileAttr {
    stat: abi::stat,
}

pub struct ReadDir {
    entries: IntoIter<DirEntry>,
}

#[derive(Clone, Debug, Default)]
pub struct DirEntry {
    name: PathBuf,
    attr: FileAttr,
    dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct OpenOptions {
    // generic
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    // system-specific
    mode: i32,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {
    accessed: Option<SystemTime>,
    modified: Option<SystemTime>,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FilePermissions {
    mode: u32,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FileType {
    mode: u32,
}

#[derive(Debug)]
pub struct DirBuilder {
    mode: u32,
}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.stat.st_size as u64
    }

    pub fn perm(&self) -> FilePermissions {
        FilePermissions { mode: (self.stat.st_mode as u32) }
    }

    pub fn file_type(&self) -> FileType {
        FileType { mode: self.stat.st_mode as u32 }
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        Ok(SystemTime::new(self.stat.st_mtime as i64, self.stat.st_mtime_nsec as i64))
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        Ok(SystemTime::new(self.stat.st_atime as i64, self.stat.st_atime_nsec as i64))
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        Ok(SystemTime::new(self.stat.st_birthtime as i64, self.stat.st_birthtime_nsec as i64))
    }
}

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        self.mode & 0o222 == 0
    }

    pub fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            // remove write permission for all classes; equivalent to `chmod a-w <file>`
            self.mode &= !0o222;
        } else {
            // add write permission for all classes; equivalent to `chmod a+w <file>`
            self.mode |= 0o222;
        }
    }
}

impl FileTimes {
    pub fn set_accessed(&mut self, t: SystemTime) {
        self.accessed = Some(t);
    }
    pub fn set_modified(&mut self, t: SystemTime) {
        self.modified = Some(t);
    }
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.is(S_IFDIR)
    }
    pub fn is_file(&self) -> bool {
        self.is(S_IFREG)
    }
    pub fn is_symlink(&self) -> bool {
        self.is(S_IFLNK)
    }

    pub fn is(&self, mode: u32) -> bool {
        self.masked() == mode
    }

    fn masked(&self) -> u32 {
        self.mode & S_IFMT
    }
}

impl core::hash::Hash for FileType {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.masked().hash(state);
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.entries, f)
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        self.entries.next().map(|d| io::Result::Ok(d))
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.dir.as_path().join(self.name.as_path())
    }

    pub fn file_name(&self) -> OsString {
        let mut s = OsString::new();
        s.push(self.name.as_path());
        s
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        Ok(self.attr)
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        Ok(self.attr.file_type())
    }
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions {
            // generic
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            // system-specific
            mode: 0o777,
        }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }
    pub fn write(&mut self, write: bool) {
        self.write = write;
    }
    pub fn append(&mut self, append: bool) {
        self.append = append;
    }
    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }
    pub fn create(&mut self, create: bool) {
        self.create = create;
    }
    pub fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }

    fn get_access_mode(&self) -> io::Result<i32> {
        match (self.read, self.write, self.append) {
            (true, false, false) => Ok(O_RDONLY),
            (false, true, false) => Ok(O_WRONLY),
            (true, true, false) => Ok(O_RDWR),
            (false, _, true) => Ok(O_WRONLY | O_APPEND),
            (true, _, true) => Ok(O_RDWR | O_APPEND),
            (false, false, false) => {
                Err(io::const_io_error!(ErrorKind::InvalidInput, "invalid access mode"))
            }
        }
    }

    fn get_creation_mode(&self) -> io::Result<i32> {
        match (self.write, self.append) {
            (true, false) => {}
            (false, false) => {
                if self.truncate || self.create || self.create_new {
                    return Err(io::const_io_error!(
                        ErrorKind::InvalidInput,
                        "invalid creation mode",
                    ));
                }
            }
            (_, true) => {
                if self.truncate && !self.create_new {
                    return Err(io::const_io_error!(
                        ErrorKind::InvalidInput,
                        "invalid creation mode",
                    ));
                }
            }
        }

        Ok(match (self.create, self.truncate, self.create_new) {
            (false, false, false) => 0,
            (true, false, false) => O_CREAT,
            (false, true, false) => O_TRUNC,
            (true, true, false) => O_CREAT | O_TRUNC,
            (_, _, true) => O_CREAT | O_EXCL,
        })
    }
}

impl File {
    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        run_path_with_cstr(path, |path| File::open_c(&path, opts))
    }

    pub fn open_c(path: &CStr, opts: &OpenOptions) -> io::Result<File> {
        let mut flags = opts.get_access_mode()?;
        flags = flags | opts.get_creation_mode()?;

        let mode;
        if flags & O_CREAT == O_CREAT {
            mode = opts.mode;
        } else {
            mode = 0;
        }

        let fd = unsafe { cvt(abi::open(path.as_ptr(), flags, mode))? };
        Ok(File(unsafe { FileDesc::from_raw_fd(fd as i32) }))
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        let fd = self.as_raw_fd();
        let mut stat: abi::stat = unsafe { mem::zeroed() };
        cvt(unsafe { abi::fstat(fd, &mut stat) })?;
        Ok(FileAttr { stat })
    }

    pub fn fsync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn datasync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn truncate(&self, _size: u64) -> io::Result<()> {
        unsupported()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        crate::io::default_read_vectored(|buf| self.read(buf), bufs)
    }

    #[inline]
    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn read_buf(&self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        crate::io::default_read_buf(|buf| self.read(buf), cursor)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        crate::io::default_write_vectored(|buf| self.write(buf), bufs)
    }

    #[inline]
    pub fn is_write_vectored(&self) -> bool {
        false
    }

    #[inline]
    pub fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (whence, pos) = match pos {
            // Casting to `i64` is fine, too large values will end up as
            // negative which will cause an error in `lseek64`.
            SeekFrom::Start(off) => (abi::SEEK_SET, off as i64),
            SeekFrom::End(off) => (abi::SEEK_END, off),
            SeekFrom::Current(off) => (abi::SEEK_CUR, off),
        };
        let n = cvt(unsafe { abi::lseek(self.as_raw_fd(), pos as isize, whence) })?;
        Ok(n as u64)
    }

    pub fn duplicate(&self) -> io::Result<File> {
        Err(Error::from_raw_os_error(22))
    }

    pub fn set_permissions(&self, _perm: FilePermissions) -> io::Result<()> {
        Err(Error::from_raw_os_error(22))
    }

    pub fn set_times(&self, _times: FileTimes) -> io::Result<()> {
        Err(Error::from_raw_os_error(22))
    }
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder { mode: 0o777 }
    }

    pub fn mkdir(&self, p: &Path) -> io::Result<()> {
        run_path_with_cstr(p, |p| cvt(unsafe { abi::mkdir(p.as_ptr(), self.mode) }).map(|_| ()))
    }

    /*
    pub fn set_mode(&mut self, mode: u32) {
        self.mode = mode as mode_t;
    }
    */
}

impl AsInner<FileDesc> for File {
    #[inline]
    fn as_inner(&self) -> &FileDesc {
        &self.0
    }
}

impl AsInnerMut<FileDesc> for File {
    #[inline]
    fn as_inner_mut(&mut self) -> &mut FileDesc {
        &mut self.0
    }
}

impl IntoInner<FileDesc> for File {
    fn into_inner(self) -> FileDesc {
        self.0
    }
}

impl FromInner<FileDesc> for File {
    fn from_inner(file_desc: FileDesc) -> Self {
        Self(file_desc)
    }
}

impl AsFd for File {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for File {
    #[inline]
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl IntoRawFd for File {
    fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }
}

impl FromRawFd for File {
    unsafe fn from_raw_fd(raw_fd: RawFd) -> Self {
        Self(FromRawFd::from_raw_fd(raw_fd))
    }
}

pub fn readdir(_p: &Path) -> io::Result<ReadDir> {
    unsupported()
}

pub fn unlink(path: &Path) -> io::Result<()> {
    run_path_with_cstr(path, |path| cvt(unsafe { abi::unlink(path.as_ptr()) }).map(|_| ()))
}

pub fn rename(_old: &Path, _new: &Path) -> io::Result<()> {
    unsupported()
}

pub fn set_perm(path: &Path, perm: FilePermissions) -> io::Result<()> {
    run_path_with_cstr(path, |p| {
        cvt(unsafe { abi::set_permission(p.as_ptr(), perm.mode) }).map(|_| ())
    })
}

pub fn rmdir(p: &Path) -> io::Result<()> {
    run_path_with_cstr(p, |p| {
        cvt(unsafe { abi::rmdir(p.as_ptr()) })?;
        Ok(())
    })
}

pub fn remove_dir_all(p: &Path) -> io::Result<()> {
    run_path_with_cstr(p, |p| {
        cvt(unsafe { abi::rmdir(p.as_ptr()) })?;
        Ok(())
    })
}

pub fn readlink(_p: &Path) -> io::Result<PathBuf> {
    unsupported()
}

pub fn symlink(_original: &Path, _link: &Path) -> io::Result<()> {
    unsupported()
}

pub fn link(_original: &Path, _link: &Path) -> io::Result<()> {
    unsupported()
}

pub fn stat(p: &Path) -> io::Result<FileAttr> {
    run_path_with_cstr(p, |p| {
        let mut stat: abi::stat = unsafe { mem::zeroed() };
        cvt(unsafe { abi::stat(p.as_ptr(), &mut stat) })?;
        Ok(FileAttr { stat })
    })
}

pub fn lstat(p: &Path) -> io::Result<FileAttr> {
    run_path_with_cstr(p, |p| {
        let mut stat: abi::stat = unsafe { mem::zeroed() };
        cvt(unsafe { abi::lstat(p.as_ptr(), &mut stat) })?;
        Ok(FileAttr { stat })
    })
}

pub fn canonicalize(_p: &Path) -> io::Result<PathBuf> {
    unsupported()
}
