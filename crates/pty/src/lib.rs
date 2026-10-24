use std::fs::File;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::{collections::HashMap, ffi::CString, io, os::fd::AsRawFd, path::PathBuf};

use nix::errno::Errno;
use nix::fcntl::OFlag;
use nix::libc::_exit;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::chdir;
use nix::unistd::{Pid, pipe2};
use nix::{
    libc::{TIOCSCTTY, ioctl},
    pty::{Winsize, openpty},
    unistd::{dup2_stderr, dup2_stdin, dup2_stdout, execvp, fork, setsid},
};

pub struct Pty {
    master: File,
    pid: Pid,
    status: Option<ExitStatus>,
}

pub struct PtyOptions {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub size: PtySize,
}

#[derive(Clone, Copy)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Pty {
    pub fn spawn(options: PtyOptions) -> io::Result<Self> {
        let winsize = Winsize {
            ws_row: options.size.rows,
            ws_col: options.size.cols,
            ws_xpixel: options.size.pixel_width,
            ws_ypixel: options.size.pixel_height,
        };

        let pty = openpty(&winsize, None)?;

        let program = CString::new(options.program.as_str())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let mut args = Vec::with_capacity(options.args.len() + 1);

        args.push(
            CString::new(options.program.clone())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?,
        );

        for arg in &options.args {
            args.push(
                CString::new(arg.as_str())
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?,
            );
        }

        let (error_read, error_write) = pipe2(OFlag::O_CLOEXEC)?;

        match unsafe { fork()? } {
            nix::unistd::ForkResult::Parent { child } => {
                drop(pty.slave);
                Ok(Self {
                    master: File::from(pty.master),
                    pid: child,
                    status: None,
                })
            }
            nix::unistd::ForkResult::Child => {
                drop(pty.master);
                let slave = pty.slave;

                drop(error_read);
                if let Err(errno) = setsid() {
                    child_error(&error_write, errno);
                }

                if unsafe { ioctl(slave.as_raw_fd(), TIOCSCTTY, 0) } == -1 {
                    child_error(&error_write, Errno::last());
                }

                if let Err(errno) = dup2_stdin(&slave) {
                    child_error(&error_write, errno);
                }

                if let Err(errno) = dup2_stdout(&slave) {
                    child_error(&error_write, errno);
                }

                if let Err(errno) = dup2_stderr(&slave) {
                    child_error(&error_write, errno);
                }

                if let Some(cwd) = &options.cwd
                    && let Err(errno) = chdir(cwd)
                {
                    child_error(&error_write, errno);
                }

                drop(slave);

                let Err(errno) = execvp(&program, &args);
                child_error(&error_write, errno);
            }
        }
    }

    pub fn reader(&mut self) -> &mut impl io::Read {
        &mut self.master
    }

    pub fn writer(&mut self) -> &mut impl io::Write {
        &mut self.master
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }

        let status = match waitpid(self.pid, Some(WaitPidFlag::WNOHANG))? {
            WaitStatus::StillAlive => return Ok(None),
            WaitStatus::Exited(_, code) => ExitStatus::from_raw(code << 8),
            WaitStatus::Signaled(_, signal, core_dumped) => {
                let mut raw = signal as i32;
                if core_dumped {
                    raw |= 0x80;
                }
                ExitStatus::from_raw(raw)
            }
            _ => return Ok(None),
        };

        self.status = Some(status);
        Ok(Some(status))
    }
}

fn child_error(fd: &impl std::os::fd::AsFd, errno: Errno) -> ! {
    let value = (errno as i32).to_ne_bytes();

    let _ = nix::unistd::write(fd, &value);

    unsafe {
        _exit(127);
    }
}
