use std::fs::File;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::{
    collections::HashMap,
    ffi::{CStr, CString},
    io,
    os::fd::AsRawFd,
    path::PathBuf,
};

use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
#[cfg(unix)]
use nix::unistd::Pid;
use nix::{
    libc::{TIOCSCTTY, ioctl},
    pty::{Winsize, openpty},
    unistd::{dup2_stderr, dup2_stdin, dup2_stdout, execvp, fork, setsid},
};

pub struct Pty {
    #[cfg(unix)]
    master: File,

    #[cfg(unix)]
    pid: Pid,

    #[cfg(unix)]
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

        let program = &CString::new(options.program).unwrap();

        let args: Vec<CString> = options
            .args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect();
        let args: Vec<&CStr> = args.iter().map(|arg| arg.as_c_str()).collect();

        match unsafe { fork()? } {
            nix::unistd::ForkResult::Parent { child } => {
                drop(pty.slave);
                return Ok(Self {
                    master: File::from(pty.master),
                    pid: child,
                    status: None,
                });
            }
            nix::unistd::ForkResult::Child => {
                drop(pty.master);
                let slave = pty.slave;

                setsid()?;

                if unsafe { ioctl(slave.as_raw_fd(), TIOCSCTTY, 0) } == -1 {
                    return Err(io::Error::last_os_error());
                }

                dup2_stdin(&slave)?;
                dup2_stdout(&slave)?;
                dup2_stderr(&slave)?;

                drop(slave);

                execvp(program, &args)?;
                unreachable!();
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
