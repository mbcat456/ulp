#![allow(unsafe_code)]

use std::fs::File;

#[cfg(target_os = "windows")]
mod imp {
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;

    const PAGE_READONLY: u32 = 0x02;
    const FILE_MAP_READ: u32 = 0x0004;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileMappingW(
            hFile: isize,
            lpAttributes: *const u8,
            flProtect: u32,
            dwMaximumSizeHigh: u32,
            dwMaximumSizeLow: u32,
            lpName: *const u16,
        ) -> isize;
        fn MapViewOfFile(
            hFileMappingObject: isize,
            dwDesiredAccess: u32,
            dwFileOffsetHigh: u32,
            dwFileOffsetLow: u32,
            dwNumberOfBytesToMap: usize,
        ) -> *mut u8;
        fn UnmapViewOfFile(lpBaseAddress: *const u8) -> i32;
        fn CloseHandle(hObject: isize) -> i32;
    }

    pub(super) fn map(file: &File, len: usize) -> std::io::Result<*const u8> {
        let handle = file.as_raw_handle() as isize;

        let mapping = unsafe {
            CreateFileMappingW(
                handle,
                std::ptr::null(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            )
        };
        if mapping == 0 || mapping == -1 {
            return Err(std::io::Error::last_os_error());
        }

        let ptr = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, len) };

        unsafe { CloseHandle(mapping) };
        if ptr.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(ptr)
    }

    pub(super) fn unmap(ptr: *const u8, _len: usize) {
        unsafe {
            UnmapViewOfFile(ptr);
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::os::unix::io::AsRawFd;

    const PROT_READ: i32 = 0x1;
    const MAP_PRIVATE: i32 = 0x02;

    const MAP_FAILED: *mut u8 = usize::MAX as *mut u8;

    #[link(name = "c")]
    unsafe extern "C" {
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
        fn munmap(addr: *mut u8, len: usize) -> i32;
    }

    pub(super) fn map(file: &File, len: usize) -> std::io::Result<*const u8> {
        let fd = file.as_raw_fd();

        let ptr = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_PRIVATE, fd, 0) };
        if ptr == MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(ptr)
    }

    pub(super) fn unmap(ptr: *const u8, len: usize) {
        unsafe {
            munmap(ptr as *mut u8, len);
        }
    }
}

pub struct MappedFile {
    ptr: *const u8,
    len: usize,
}

impl MappedFile {
    pub fn open(path: &str) -> std::io::Result<MappedFile> {
        let file = File::open(path)?;
        let len = file.metadata()?.len() as usize;
        let ptr = imp::map(&file, len)?;
        Ok(MappedFile { ptr, len })
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for MappedFile {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            imp::unmap(self.ptr, self.len);
        }
    }
}

unsafe impl Send for MappedFile {}
unsafe impl Sync for MappedFile {}
