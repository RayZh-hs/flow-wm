//! Observe application exits without owning or reaping the application's process.
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    sync::mpsc,
    thread,
};

enum Command {
    Watch(u64, OwnedFd),
    Unwatch(u64),
    Stop,
}

pub(crate) struct ProcessMonitor {
    commands: mpsc::Sender<Command>,
    wake: UnixStream,
    worker: Option<thread::JoinHandle<()>>,
}

impl ProcessMonitor {
    pub fn new(exited: impl Fn(u64, Result<u8, String>) + Send + 'static) -> io::Result<Self> {
        let (wake, reader) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        reader.set_nonblocking(true)?;
        let (commands, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("window-processes".into())
            .spawn(move || observe(reader, receiver, exited))?;
        Ok(Self {
            commands,
            wake,
            worker: Some(worker),
        })
    }

    pub fn watch(&mut self, token: u64, pid: i32) -> io::Result<()> {
        if pid <= 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid application PID",
            ));
        }
        // Acquire the stable process reference while its window is still present.
        let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
        // Fail promptly on kernels without PIDFD_GET_INFO. The exit mask is only
        // populated once the real parent has reaped the process (Linux >=6.15).
        exit_code(&fd)?;
        self.send(Command::Watch(token, fd))
    }

    pub fn unwatch(&mut self, token: u64) {
        let _ = self.send(Command::Unwatch(token));
    }

    fn send(&mut self, command: Command) -> io::Result<()> {
        self.commands
            .send(command)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Process monitor stopped"))?;
        loop {
            match self.wake.write(&[1]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                result => return result.map(|_| ()),
            }
        }
    }
}

impl Drop for ProcessMonitor {
    fn drop(&mut self) {
        let _ = self.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn exit_code(fd: &OwnedFd) -> io::Result<Option<u8>> {
    let mut info: libc::pidfd_info = unsafe { std::mem::zeroed() };
    info.mask = libc::PIDFD_INFO_EXIT.into();
    if unsafe { libc::ioctl(fd.as_raw_fd(), libc::PIDFD_GET_INFO, &mut info) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if info.mask & u64::from(libc::PIDFD_INFO_EXIT) == 0 {
        return Ok(None);
    }
    let status = info.exit_code;
    let code = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        return Err(io::Error::other("Unexpected application wait status"));
    };
    u8::try_from(code).map(Some).map_err(io::Error::other)
}

fn observe(
    mut wake: UnixStream,
    commands: mpsc::Receiver<Command>,
    exited: impl Fn(u64, Result<u8, String>),
) {
    let mut processes = BTreeMap::<u64, OwnedFd>::new();
    loop {
        let tokens: Vec<_> = processes.keys().copied().collect();
        let mut fds = vec![libc::pollfd {
            fd: wake.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        fds.extend(processes.values().map(|fd| libc::pollfd {
            fd: fd.as_raw_fd(),
            // POLLIN means exited but possibly still a zombie. POLLHUP is
            // reported regardless of events and means exit info is finalized.
            // Waiting for HUP avoids spinning until the parent reaps the child.
            events: 0,
            revents: 0,
        }));
        let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if result < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            let error = io::Error::last_os_error().to_string();
            for token in processes.keys() {
                exited(*token, Err(error.clone()));
            }
            return;
        }
        if fds[0].revents != 0 {
            let mut bytes = [0; 256];
            while wake.read(&mut bytes).is_ok_and(|n| n > 0) {}
            for command in commands.try_iter() {
                match command {
                    Command::Watch(token, fd) => {
                        processes.insert(token, fd);
                    }
                    Command::Unwatch(token) => {
                        processes.remove(&token);
                    }
                    Command::Stop => return,
                }
            }
        }
        for (token, event) in tokens.into_iter().zip(&fds[1..]) {
            if event.revents != 0
                && let Some(fd) = processes.remove(&token)
            {
                let result = exit_code(&fd)
                    .and_then(|code| code.ok_or_else(|| io::Error::other("Application exit status unavailable; Linux 6.15 or newer is required")))
                    .map_err(|e| e.to_string());
                exited(token, result);
            }
        }
    }
}
