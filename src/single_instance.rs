use std::io;

pub struct SingleInstanceGuard {
    #[cfg(target_os = "windows")]
    handle: *mut core::ffi::c_void,
}

impl SingleInstanceGuard {
    pub fn try_acquire(name: &str) -> io::Result<Option<Self>> {
        #[cfg(target_os = "windows")]
        {
            windows_impl::try_acquire(name)
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = name;
            Ok(Some(Self {}))
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                windows_impl::close_handle(self.handle);
            }
            self.handle = core::ptr::null_mut();
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::SingleInstanceGuard;
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt;

    const ERROR_ALREADY_EXISTS: u32 = 183;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(
            lp_mutex_attributes: *mut core::ffi::c_void,
            b_initial_owner: i32,
            lp_name: *const u16,
        ) -> *mut core::ffi::c_void;
        fn GetLastError() -> u32;
        fn CloseHandle(h_object: *mut core::ffi::c_void) -> i32;
    }

    pub(super) fn try_acquire(name: &str) -> io::Result<Option<SingleInstanceGuard>> {
        let mut wide_name: Vec<u16> = OsStr::new(name).encode_wide().collect();
        wide_name.push(0);

        let handle = unsafe { CreateMutexW(core::ptr::null_mut(), 0, wide_name.as_ptr()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }

        let last_error = unsafe { GetLastError() };
        if last_error == ERROR_ALREADY_EXISTS {
            unsafe {
                close_handle(handle);
            }
            return Ok(None);
        }

        Ok(Some(SingleInstanceGuard { handle }))
    }

    pub(super) unsafe fn close_handle(handle: *mut core::ffi::c_void) {
        let _ = unsafe { CloseHandle(handle) };
    }
}
