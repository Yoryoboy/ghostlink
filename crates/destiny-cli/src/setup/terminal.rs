//! Prompt-scoped interrupt handling. Console restores termios before raising
//! SIGINT on Unix; let its Interrupted error unwind and restore the cursor.
use super::Aborted;

#[cfg(unix)]
pub struct Guard(libc::sigaction);
#[cfg(unix)]
extern "C" fn interrupted(_: libc::c_int) {}
#[cfg(unix)]
impl Guard {
    pub fn new() -> Result<Self, Aborted> {
        // SAFETY: fully initialized sigaction, valid pointers, signal-safe no-op.
        unsafe {
            let mut previous = std::mem::zeroed();
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = interrupted as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(libc::SIGINT, &action, &mut previous) != 0 {
                return Err(Aborted);
            }
            Ok(Self(previous))
        }
    }
}
#[cfg(unix)]
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = dialoguer::console::Term::stdout().show_cursor();
        // SAFETY: restore the exact previous process disposition.
        unsafe {
            libc::sigaction(libc::SIGINT, &self.0, std::ptr::null_mut());
        }
    }
}

// Windows console processes Ctrl+C outside ReadConsoleInputW. A prompt-scoped
// handler reports cancellation and exits, without running login or saving a
// marker. No echo suppression is used by these prompts.
#[cfg(windows)]
mod windows {
    use super::Aborted;
    type Handle = *mut std::ffi::c_void;
    #[repr(C)]
    struct Cursor {
        size: u32,
        visible: i32,
    }
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
        fn GetStdHandle(kind: u32) -> Handle;
        fn WriteFile(
            handle: Handle,
            buffer: *const u8,
            size: u32,
            written: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
        fn GetConsoleCursorInfo(handle: Handle, info: *mut Cursor) -> i32;
        fn SetConsoleCursorInfo(handle: Handle, info: *const Cursor) -> i32;
        fn ExitProcess(code: u32) -> !;
    }
    unsafe extern "system" fn cancel(kind: u32) -> i32 {
        if kind != 0 {
            return 0;
        }
        // SAFETY: OS standard handles and fixed buffers; no Rust allocation or
        // locks on the console handler thread. Marker is written only after prompts.
        unsafe {
            let out = GetStdHandle(-11_i32 as u32);
            let mut cursor = Cursor {
                size: 25,
                visible: 1,
            };
            if GetConsoleCursorInfo(out, &mut cursor) != 0 {
                cursor.visible = 1;
                SetConsoleCursorInfo(out, &cursor);
            }
            let message = b"\r\nSetup cancelled; completed steps were kept.\r\n";
            let mut written = 0;
            WriteFile(
                GetStdHandle(-12_i32 as u32),
                message.as_ptr(),
                message.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            );
            ExitProcess(130)
        }
    }
    pub struct Guard;
    impl Guard {
        pub fn new() -> Result<Self, Aborted> {
            // SAFETY: handler has the required system ABI and lives forever.
            if unsafe { SetConsoleCtrlHandler(Some(cancel), 1) } == 0 {
                Err(Aborted)
            } else {
                Ok(Self)
            }
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: remove only our own registered callback.
            unsafe {
                SetConsoleCtrlHandler(Some(cancel), 0);
            }
            let _ = dialoguer::console::Term::stdout().show_cursor();
        }
    }
}
#[cfg(windows)]
pub use windows::Guard;
#[cfg(not(any(unix, windows)))]
pub struct Guard;
#[cfg(not(any(unix, windows)))]
impl Guard {
    pub fn new() -> Result<Self, Aborted> {
        Err(Aborted)
    }
}
