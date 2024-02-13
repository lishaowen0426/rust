#![stable(feature = "rust1", since = "1.0.0")]

use crate::fs::{self};
/// Hermit-specific extensions to [`fs::Metadata`].
#[stable(feature = "metadata_ext", since = "1.1.0")]
pub trait MetadataExt {
    /// Returns the ID of the device containing the file.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::io;
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let dev_id = meta.dev();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn dev(&self) -> u64;
    /// Returns the inode number.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let inode = meta.ino();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn ino(&self) -> u64;
    /// Returns the rights applied to this file.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let mode = meta.mode();
    ///     let user_has_write_access      = mode & 0o200;
    ///     let user_has_read_write_access = mode & 0o600;
    ///     let group_has_read_access      = mode & 0o040;
    ///     let others_have_exec_access    = mode & 0o001;
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn mode(&self) -> u32;
    /// Returns the number of hard links pointing to this file.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let nb_hard_links = meta.nlink();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn nlink(&self) -> u64;
    /// Returns the user ID of the owner of this file.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let user_id = meta.uid();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn uid(&self) -> u32;
    /// Returns the group ID of the owner of this file.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let group_id = meta.gid();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn gid(&self) -> u32;
    /// Returns the device ID of this file (if it is a special one).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let device_id = meta.rdev();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn rdev(&self) -> u64;
    /// Returns the total size of this file in bytes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let file_size = meta.size();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn size(&self) -> u64;
    /// Returns the last access time of the file, in seconds since Unix Epoch.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let last_access_time = meta.atime();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn atime(&self) -> i64;
    /// Returns the last access time of the file, in nanoseconds since [`atime`].
    ///
    /// [`atime`]: MetadataExt::atime
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let nano_last_access_time = meta.atime_nsec();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn atime_nsec(&self) -> i64;
    /// Returns the last modification time of the file, in seconds since Unix Epoch.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let last_modification_time = meta.mtime();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn mtime(&self) -> i64;
    /// Returns the last modification time of the file, in nanoseconds since [`mtime`].
    ///
    /// [`mtime`]: MetadataExt::mtime
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let nano_last_modification_time = meta.mtime_nsec();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn mtime_nsec(&self) -> i64;
    /// Returns the last status change time of the file, in seconds since Unix Epoch.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let last_status_change_time = meta.ctime();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn ctime(&self) -> i64;
    /// Returns the last status change time of the file, in nanoseconds since [`ctime`].
    ///
    /// [`ctime`]: MetadataExt::ctime
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let nano_last_status_change_time = meta.ctime_nsec();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn ctime_nsec(&self) -> i64;
    /// Returns the block size for filesystem I/O.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let block_size = meta.blksize();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn blksize(&self) -> u64;
    /// Returns the number of blocks allocated to the file, in 512-byte units.
    ///
    /// Please note that this may be smaller than `st_size / 512` when the file has holes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs;
    /// use std::os::unix::fs::MetadataExt;
    /// use std::io;
    ///
    /// fn main() -> io::Result<()> {
    ///     let meta = fs::metadata("some_file")?;
    ///     let blocks = meta.blocks();
    ///     Ok(())
    /// }
    /// ```
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn blocks(&self) -> u64;
    #[cfg(target_os = "vxworks")]
    #[stable(feature = "metadata_ext", since = "1.1.0")]
    fn attrib(&self) -> u8;
}

use crate::sys_common::AsInner;
#[stable(feature = "metadata_ext", since = "1.1.0")]
impl MetadataExt for fs::Metadata {
    fn dev(&self) -> u64 {
        self.as_inner().as_inner().st_dev as u64
    }
    fn ino(&self) -> u64 {
        self.as_inner().as_inner().st_ino as u64
    }
    fn mode(&self) -> u32 {
        self.as_inner().as_inner().st_mode as u32
    }
    fn nlink(&self) -> u64 {
        self.as_inner().as_inner().st_nlink as u64
    }
    fn uid(&self) -> u32 {
        self.as_inner().as_inner().st_uid as u32
    }
    fn gid(&self) -> u32 {
        self.as_inner().as_inner().st_gid as u32
    }
    fn rdev(&self) -> u64 {
        self.as_inner().as_inner().st_rdev as u64
    }
    fn size(&self) -> u64 {
        self.as_inner().as_inner().st_size as u64
    }
    fn atime(&self) -> i64 {
        self.as_inner().as_inner().st_atime as i64
    }
    fn atime_nsec(&self) -> i64 {
        self.as_inner().as_inner().st_atime_nsec as i64
    }
    fn mtime(&self) -> i64 {
        self.as_inner().as_inner().st_mtime as i64
    }
    fn mtime_nsec(&self) -> i64 {
        self.as_inner().as_inner().st_mtime_nsec as i64
    }
    fn ctime(&self) -> i64 {
        self.as_inner().as_inner().st_ctime as i64
    }
    fn ctime_nsec(&self) -> i64 {
        self.as_inner().as_inner().st_ctime_nsec as i64
    }
    fn blksize(&self) -> u64 {
        self.as_inner().as_inner().st_blksize as u64
    }
    fn blocks(&self) -> u64 {
        self.as_inner().as_inner().st_blocks as u64
    }
    #[cfg(target_os = "vxworks")]
    fn attrib(&self) -> u8 {
        0
    }
}
