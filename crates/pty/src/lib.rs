use std::fs::File;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::{collections::HashMap, ffi::CString, io, os::fd::AsRawFd, path::PathBuf};

use nix::errno::Errno;
use nix::fcntl::OFlag;
use nix::libc::_exit;
use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::chdir;
use nix::unistd::{Pid, pipe2};
use nix::{
    libc::{TIOCSCTTY, ioctl},
    pty::{Winsize, openpty},
    unistd::{dup2_stderr, dup2_stdin, dup2_stdout, execvpe, fork, setsid},
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

        let mut environment: HashMap<_, _> = std::env::vars_os().collect();
        for (key, value) in &options.env {
            environment.insert(key.into(), value.into());
        }
        let environment = environment
            .into_iter()
            .map(|(key, value)| {
                let mut entry = key.into_encoded_bytes();
                entry.push(b'=');
                entry.extend(value.into_encoded_bytes());
                CString::new(entry).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
            })
            .collect::<io::Result<Vec<_>>>()?;

        let (error_read, error_write) = pipe2(OFlag::O_CLOEXEC)?;

        match unsafe { fork()? } {
            nix::unistd::ForkResult::Parent { child } => {
                drop(pty.slave);
                drop(error_write);
                let mut error_read = File::from(error_read);
                let mut bytes = [0; 4];
                match error_read.read_exact(&mut bytes) {
                    Ok(()) => {
                        let _ = waitpid(child, None);
                        return Err(io::Error::from_raw_os_error(i32::from_ne_bytes(bytes)));
                    }
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
                    Err(error) => return Err(error),
                }
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

                let Err(errno) = execvpe(&program, &args, &environment);
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

    pub fn clone_reader(&self) -> io::Result<File> {
        self.master.try_clone()
    }

    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        let winsize = Winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        let result =
            unsafe { nix::libc::ioctl(self.master.as_raw_fd(), nix::libc::TIOCSWINSZ, &winsize) };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
        kill(self.pid, Signal::SIGWINCH)?;
        Ok(())
    }

    pub fn terminate(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_none() {
            kill(self.pid, Signal::SIGHUP)?;
            let _ = waitpid(self.pid, None)?;
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn options(program: &str) -> PtyOptions {
        PtyOptions {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            size: PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 800,
                pixel_height: 480,
            },
        }
    }

    #[test]
    fn reports_exec_failure() {
        let result = Pty::spawn(options("/not/a/real/executable"));
        assert_eq!(
            result.err().and_then(|error| error.raw_os_error()),
            Some(nix::libc::ENOENT)
        );
    }

    #[test]
    fn passes_environment_to_child() {
        let mut options = options("/bin/sh");
        options.args = vec!["-c".into(), "printf '%s' \"$RUTERM_PTY_TEST\"".into()];
        options
            .env
            .insert("RUTERM_PTY_TEST".into(), "passed".into());
        let mut pty = Pty::spawn(options).unwrap();
        let mut output = [0; 64];
        let count = pty.reader().read(&mut output).unwrap();
        assert_eq!(&output[..count], b"passed");
        pty.terminate().unwrap();
    }
}
