use std::{
    fs::{File, OpenOptions},
    os::fd::AsRawFd,
    path::Path,
};

use nix::fcntl;

pub struct FileGuard {
    file: File,
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        _ = fcntl::flock(self.file.as_raw_fd(), nix::fcntl::FlockArg::Unlock);
    }
}

pub fn flock(path: &Path) -> std::io::Result<FileGuard> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    fcntl::flock(
        file.as_raw_fd(),
        nix::fcntl::FlockArg::LockExclusiveNonblock,
    )?;
    Ok(FileGuard { file })
}
