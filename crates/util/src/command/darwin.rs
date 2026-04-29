use mach2::exception_types::{
    EXC_MASK_ALL, EXCEPTION_DEFAULT, exception_behavior_t, exception_mask_t,
};
use mach2::port::{MACH_PORT_NULL, mach_port_t};
use mach2::thread_status::{THREAD_STATE_NONE, thread_state_flavor_t};
use smol::Unblock;
use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::FromRawFd;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::ptr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stdio {
    /// A new pipe should be arranged to connect the parent and child processes.
    #[default]
    Piped,
    /// The child inherits from the corresponding parent descriptor.
    Inherit,
    /// This stream will be ignored (redirected to `/dev/null`).
    Null,
}

impl Stdio {
    pub fn piped() -> Self {
        Self::Piped
    }

    pub fn inherit() -> Self {
        Self::Inherit
    }

    pub fn null() -> Self {
        Self::Null
    }
}

unsafe extern "C" {
    fn posix_spawnattr_setexceptionports_np(
        attr: *mut libc::posix_spawnattr_t,
        mask: exception_mask_t,
        new_port: mach_port_t,
        behavior: exception_behavior_t,
        new_flavor: thread_state_flavor_t,
    ) -> libc::c_int;

    fn posix_spawn_file_actions_addchdir_np(
        file_actions: *mut libc::posix_spawn_file_actions_t,
        path: *const libc::c_char,
    ) -> libc::c_int;

    fn posix_spawn_file_actions_addinherit_np(
        file_actions: *mut libc::posix_spawn_file_actions_t,
        filedes: libc::c_int,
    ) -> libc::c_int;

    static environ: *const *mut libc::c_char;
}

#[derive(Debug)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    envs: BTreeMap<OsString, Option<OsString>>,
    env_clear: bool,
    current_dir: Option<PathBuf>,
    stdin_cfg: Option<Stdio>,
    stdout_cfg: Option<Stdio>,
    stderr_cfg: Option<Stdio>,
    kill_on_drop: bool,
}

impl Command {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            envs: BTreeMap::new(),
            env_clear: false,
            current_dir: None,
            stdin_cfg: None,
            stdout_cfg: None,
            stderr_cfg: None,
            kill_on_drop: false,
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> &mut Self {
        self.envs
            .insert(key.as_ref().to_owned(), Some(val.as_ref().to_owned()));
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (key, val) in vars {
            self.envs
                .insert(key.as_ref().to_owned(), Some(val.as_ref().to_owned()));
        }
        self
    }

    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        let key = key.as_ref().to_owned();
        if self.env_clear {
            self.envs.remove(&key);
        } else {
            self.envs.insert(key, None);
        }
        self
    }

    pub fn env_clear(&mut self) -> &mut Self {
        self.env_clear = true;
        self.envs.clear();
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.current_dir = Some(dir.as_ref().to_owned());
        self
    }

    pub fn stdin(&mut self, cfg: Stdio) -> &mut Self {
        self.stdin_cfg = Some(cfg);
        self
    }

    pub fn stdout(&mut self, cfg: Stdio) -> &mut Self {
        self.stdout_cfg = Some(cfg);
        self
    }

    pub fn stderr(&mut self, cfg: Stdio) -> &mut Self {
        self.stderr_cfg = Some(cfg);
        self
    }

    pub fn kill_on_drop(&mut self, kill_on_drop: bool) -> &mut Self {
        self.kill_on_drop = kill_on_drop;
        self
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        let current_dir = self
            .current_dir
            .as_deref()
            .unwrap_or_else(|| Path::new("."));

        // Optimization: if no environment modifications were requested, pass None
        // to spawn_posix so it uses the `environ` global directly, avoiding a
        // full copy of the environment. This matches std::process::Command behavior.
        let envs = if self.env_clear || !self.envs.is_empty() {
            let mut result = BTreeMap::<OsString, OsString>::new();
            if !self.env_clear {
                for (key, val) in std::env::vars_os() {
                    result.insert(key, val);
                }
            }
            for (key, maybe_val) in &self.envs {
                if let Some(val) = maybe_val {
                    result.insert(key.clone(), val.clone());
                } else {
                    result.remove(key);
                }
            }
            Some(result.into_iter().collect::<Vec<_>>())
        } else {
            None
        };

        spawn_posix_spawn(
            &self.program,
            &self.args,
            current_dir,
            envs.as_deref(),
            self.stdin_cfg.unwrap_or_default(),
            self.stdout_cfg.unwrap_or_default(),
            self.stderr_cfg.unwrap_or_default(),
            self.kill_on_drop,
        )
    }

    pub async fn output(&mut self) -> io::Result<Output> {
        self.stdin_cfg.get_or_insert(Stdio::null());
        self.stdout_cfg.get_or_insert(Stdio::piped());
        self.stderr_cfg.get_or_insert(Stdio::piped());

        let child = self.spawn()?;
        child.output().await
    }

    pub async fn status(&mut self) -> io::Result<ExitStatus> {
        let mut child = self.spawn()?;
        child.status().await
    }
}

#[derive(Debug)]
pub struct Child {
    pid: libc::pid_t,
    pub stdin: Option<Unblock<std::fs::File>>,
    pub stdout: Option<Unblock<std::fs::File>>,
    pub stderr: Option<Unblock<std::fs::File>>,
    kill_on_drop: bool,
    status: Option<ExitStatus>,
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.kill_on_drop && self.status.is_none() {
            let _ = self.kill();
        }
    }
}

impl Child {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn kill(&mut self) -> io::Result<()> {
        let result = unsafe { libc::kill(self.pid, libc::SIGKILL) };
        if result == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn try_status(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }

        let mut status: libc::c_int = 0;
        let result = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };

        if result == -1 {
            Err(io::Error::last_os_error())
        } else if result == 0 {
            Ok(None)
        } else {
            let exit_status = ExitStatus::from_raw(status);
            self.status = Some(exit_status);
            Ok(Some(exit_status))
        }
    }

    pub fn status(
        &mut self,
    ) -> impl std::future::Future<Output = io::Result<ExitStatus>> + Send + 'static {
        self.stdin.take();

        let pid = self.pid;
        let cached_status = self.status;

        async move {
            if let Some(status) = cached_status {
                return Ok(status);
            }

            smol::unblock(move || {
                let mut status: libc::c_int = 0;
                let result = unsafe { libc::waitpid(pid, &mut status, 0) };
                if result == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(ExitStatus::from_raw(status))
                }
            })
            .await
        }
    }

    pub async fn output(mut self) -> io::Result<Output> {
        use futures_lite::AsyncReadExt;

        let status = self.status();

        let stdout = self.stdout.take();
        let stdout_future = async move {
            let mut data = Vec::new();
            if let Some(mut stdout) = stdout {
                stdout.read_to_end(&mut data).await?;
            }
            io::Result::Ok(data)
        };

        let stderr = self.stderr.take();
        let stderr_future = async move {
            let mut data = Vec::new();
            if let Some(mut stderr) = stderr {
                stderr.read_to_end(&mut data).await?;
            }
            io::Result::Ok(data)
        };

        let (stdout_data, stderr_data) =
            futures_lite::future::try_zip(stdout_future, stderr_future).await?;
        let status = status.await?;

        Ok(Output {
            status,
            stdout: stdout_data,
            stderr: stderr_data,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_posix_spawn(
    program: &OsStr,
    args: &[OsString],
    current_dir: &Path,
    envs: Option<&[(OsString, OsString)]>,
    stdin_cfg: Stdio,
    stdout_cfg: Stdio,
    stderr_cfg: Stdio,
    kill_on_drop: bool,
) -> io::Result<Child> {
    let program_cstr = CString::new(program.as_bytes()).map_err(|_| invalid_input_error())?;

    let current_dir_cstr =
        CString::new(current_dir.as_os_str().as_bytes()).map_err(|_| invalid_input_error())?;

    let mut argv_cstrs = vec![program_cstr.clone()];
    for arg in args {
        let cstr = CString::new(arg.as_bytes()).map_err(|_| invalid_input_error())?;
        argv_cstrs.push(cstr);
    }
    let mut argv_ptrs: Vec<*mut libc::c_char> = argv_cstrs
        .iter()
        .map(|s| s.as_ptr() as *mut libc::c_char)
        .collect();
    argv_ptrs.push(ptr::null_mut());

    let envp: Vec<CString> = if let Some(envs) = envs {
        envs.iter()
            .map(|(key, value)| {
                let mut env_str = key.as_bytes().to_vec();
                env_str.push(b'=');
                env_str.extend_from_slice(value.as_bytes());
                CString::new(env_str)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid_input_error())?
    } else {
        Vec::new()
    };
    let mut envp_ptrs: Vec<*mut libc::c_char> = envp
        .iter()
        .map(|s| s.as_ptr() as *mut libc::c_char)
        .collect();
    envp_ptrs.push(ptr::null_mut());

    let (stdin_read, stdin_write) = match stdin_cfg {
        Stdio::Piped => {
            let (r, w) = create_pipe()?;
            (Some(r), Some(w))
        }
        Stdio::Null => {
            let fd = open_dev_null(libc::O_RDONLY)?;
            (Some(fd), None)
        }
        Stdio::Inherit => (None, None),
    };

    let (stdout_read, stdout_write) = match stdout_cfg {
        Stdio::Piped => {
            let (r, w) = create_pipe()?;
            (Some(r), Some(w))
        }
        Stdio::Null => {
            let fd = open_dev_null(libc::O_WRONLY)?;
            (None, Some(fd))
        }
        Stdio::Inherit => (None, None),
    };

    let (stderr_read, stderr_write) = match stderr_cfg {
        Stdio::Piped => {
            let (r, w) = create_pipe()?;
            (Some(r), Some(w))
        }
        Stdio::Null => {
            let fd = open_dev_null(libc::O_WRONLY)?;
            (None, Some(fd))
        }
        Stdio::Inherit => (None, None),
    };

    let mut attr: libc::posix_spawnattr_t = ptr::null_mut();
    let mut file_actions: libc::posix_spawn_file_actions_t = ptr::null_mut();

    unsafe {
        cvt_nz(libc::posix_spawnattr_init(&mut attr))?;
        cvt_nz(libc::posix_spawn_file_actions_init(&mut file_actions))?;

        cvt_nz(libc::posix_spawnattr_setflags(
            &mut attr,
            libc::POSIX_SPAWN_CLOEXEC_DEFAULT as libc::c_short,
        ))?;

        cvt_nz(posix_spawnattr_setexceptionports_np(
            &mut attr,
            EXC_MASK_ALL,
            MACH_PORT_NULL,
            EXCEPTION_DEFAULT as exception_behavior_t,
            THREAD_STATE_NONE,
        ))?;

        cvt_nz(posix_spawn_file_actions_addchdir_np(
            &mut file_actions,
            current_dir_cstr.as_ptr(),
        ))?;

        if let Some(fd) = stdin_read {
            cvt_nz(libc::posix_spawn_file_actions_adddup2(
                &mut file_actions,
                fd,
                libc::STDIN_FILENO,
            ))?;
            cvt_nz(posix_spawn_file_actions_addinherit_np(
                &mut file_actions,
                libc::STDIN_FILENO,
            ))?;
        }

        if let Some(fd) = stdout_write {
            cvt_nz(libc::posix_spawn_file_actions_adddup2(
                &mut file_actions,
                fd,
                libc::STDOUT_FILENO,
            ))?;
            cvt_nz(posix_spawn_file_actions_addinherit_np(
                &mut file_actions,
                libc::STDOUT_FILENO,
            ))?;
        }

        if let Some(fd) = stderr_write {
            cvt_nz(libc::posix_spawn_file_actions_adddup2(
                &mut file_actions,
                fd,
                libc::STDERR_FILENO,
            ))?;
            cvt_nz(posix_spawn_file_actions_addinherit_np(
                &mut file_actions,
                libc::STDERR_FILENO,
            ))?;
        }

        let mut pid: libc::pid_t = 0;

        let spawn_result = libc::posix_spawnp(
            &mut pid,
            program_cstr.as_ptr(),
            &file_actions,
            &attr,
            argv_ptrs.as_ptr(),
            if envs.is_some() {
                envp_ptrs.as_ptr()
            } else {
                environ
            },
        );

        libc::posix_spawnattr_destroy(&mut attr);
        libc::posix_spawn_file_actions_destroy(&mut file_actions);

        if let Some(fd) = stdin_read {
            libc::close(fd);
        }
        if let Some(fd) = stdout_write {
            libc::close(fd);
        }
        if let Some(fd) = stderr_write {
            libc::close(fd);
        }

        cvt_nz(spawn_result)?;

        Ok(Child {
            pid,
            stdin: stdin_write.map(|fd| Unblock::new(std::fs::File::from_raw_fd(fd))),
            stdout: stdout_read.map(|fd| Unblock::new(std::fs::File::from_raw_fd(fd))),
            stderr: stderr_read.map(|fd| Unblock::new(std::fs::File::from_raw_fd(fd))),
            kill_on_drop,
            status: None,
        })
    }
}

fn create_pipe() -> io::Result<(libc::c_int, libc::c_int)> {
    let mut fds: [libc::c_int; 2] = [0; 2];
    let result = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

fn open_dev_null(flags: libc::c_int) -> io::Result<libc::c_int> {
    let fd = unsafe { libc::open(c"/dev/null".as_ptr() as *const libc::c_char, flags) };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// Zero means `Ok()`, all other values are treated as raw OS errors. Does not look at `errno`.
/// Mirrored after Rust's std `cvt_nz` function.
fn cvt_nz(error: libc::c_int) -> io::Result<()> {
    if error == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(error))
    }
}

fn invalid_input_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid argument: path or argument contains null byte",
    )
}
#[cfg(test)]
#[path = "../../../../testing/util/command/darwin/tests.rs"]
mod tests;
