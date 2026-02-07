use std::{
    fs::{File, OpenOptions},
    io::{Error, Result},
    mem,
    os::windows::io::AsRawHandle,
    path::Path,
};

use windows_sys::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{
        LockFileEx, UnlockFile, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    },
};

pub struct FileGuard {
    file: File,
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        unlock(&self.file).unwrap();
    }
}

pub fn flock(path: &Path) -> Result<FileGuard> {
    let file = OpenOptions::new().write(true).create(true).open(path)?;
    lock_file(&file, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)?;
    Ok(FileGuard { file })
}

fn lock_file(file: &File, flags: u32) -> Result<()> {
    unsafe {
        let mut overlapped = mem::zeroed();
        let ret = LockFileEx(
            file.as_raw_handle() as HANDLE,
            flags,
            0,
            !0,
            !0,
            &mut overlapped,
        );
        if ret == 0 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn unlock(file: &File) -> Result<()> {
    unsafe {
        let ret = UnlockFile(file.as_raw_handle() as HANDLE, 0, 0, !0, !0);
        if ret == 0 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
