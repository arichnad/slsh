use std::fs::File;
use std::io;
use std::os::unix::io::FromRawFd;

use crate::environment::*;
use crate::eval::*;
use crate::types::*;

pub trait IsMinusOne {
    fn is_minus_one(&self) -> bool;
}

macro_rules! impl_is_minus_one {
    ($($t:ident)*) => ($(impl IsMinusOne for $t {
        fn is_minus_one(&self) -> bool {
            *self == -1
        }
    })*)
}

impl_is_minus_one! { i8 i16 i32 i64 isize }

pub fn cvt<T: IsMinusOne>(t: T) -> Result<T, LispError> {
    if t.is_minus_one() {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(t)
    }
}

pub fn anon_pipe() -> Result<(i32, i32), LispError> {
    let mut fds = [0; 2];

    /*#[cfg(any(
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "redox"
    ))] {*/
    cvt(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) })?;
    Ok((fds[0], fds[1]))
    /*} else {
        cvt(unsafe { libc::pipe(fds.as_mut_ptr()) })?;

        let fd0 = FileDesc::new(fds[0]);
        let fd1 = FileDesc::new(fds[1]);
        fd0.set_cloexec()?;
        fd1.set_cloexec()?;
        Ok((AnonPipe(fd0), AnonPipe(fd1)))
    }*/
}

pub fn fork(
    environment: &mut Environment,
    exp: Expression,
    stdin: Option<libc::c_int>,
    stdout: Option<libc::c_int>,
) -> Result<u32, LispError> {
    let result = unsafe { cvt(libc::fork())? };

    let pid = unsafe {
        match result {
            0 => {
                if let Some(stdin) = stdin {
                    cvt(libc::dup2(stdin, 0))?;
                    cvt(libc::close(stdin))?;
                }
                if let Some(stdout) = stdout {
                    cvt(libc::dup2(stdout, 1))?;
                    cvt(libc::close(stdout))?;
                }
                environment.run_background = false;
                environment.jobs.borrow_mut().clear();
                environment.stopped_procs.borrow_mut().clear();
                environment.grab_proc_output = false;
                environment.pipe_pgid = None;
                environment.terminal_fd = if let Ok(fd) = cvt(libc::dup(0)) {
                    fd
                } else {
                    0
                };
                environment.is_tty = false;
                match eval(environment, exp) {
                    Ok(_) => libc::_exit(0),
                    Err(_) => libc::_exit(1),
                }
            }
            n => n as u32,
        }
    };
    unsafe {
        if let Some(stdin) = stdin {
            cvt(libc::close(stdin))?;
        }
        if let Some(stdout) = stdout {
            cvt(libc::close(stdout))?;
        }
    }
    Ok(pid)
}

pub fn dup_fd(fd: i32) -> Result<i32, LispError> {
    Ok(unsafe { cvt(libc::dup(fd))? })
}

pub fn replace_fd(new_fd: i32, fd: i32) -> Result<i32, LispError> {
    Ok(unsafe {
        let old = cvt(libc::dup(fd))?;
        cvt(libc::dup2(new_fd, fd))?;
        cvt(libc::close(new_fd))?;
        old
    })
}

pub fn replace_stdin(new_stdin: i32) -> Result<i32, LispError> {
    replace_fd(new_stdin, 0)
}

pub fn replace_stdout(new_stdout: i32) -> Result<i32, LispError> {
    replace_fd(new_stdout, 1)
}

pub fn replace_stderr(new_stderr: i32) -> Result<i32, LispError> {
    replace_fd(new_stderr, 2)
}

pub fn dup_stdin(new_stdin: i32) -> Result<(), LispError> {
    unsafe {
        cvt(libc::dup2(new_stdin, 0))?;
        cvt(libc::close(new_stdin))?;
    }
    Ok(())
}

pub fn dup_stdout(new_stdout: i32) -> Result<(), LispError> {
    unsafe {
        cvt(libc::dup2(new_stdout, 1))?;
        cvt(libc::close(new_stdout))?;
    }
    Ok(())
}

pub fn dup_stderr(new_stderr: i32) -> Result<(), LispError> {
    unsafe {
        cvt(libc::dup2(new_stderr, 2))?;
        cvt(libc::close(new_stderr))?;
    }
    Ok(())
}

pub fn close_fd(fd: i32) -> Result<(), LispError> {
    unsafe {
        cvt(libc::close(fd))?;
    }
    Ok(())
}

pub fn fd_to_file(fd: i32) -> File {
    unsafe { File::from_raw_fd(fd) }
}