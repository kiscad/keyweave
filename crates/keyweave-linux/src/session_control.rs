//! Same-user nonblocking local session status/stop transport. No device APIs.
#![allow(unsafe_code)]
use keyweave_progress::session_control::{
    Command, Outcome, Phase, REPLY_BYTES, REQUEST_BYTES, Reply, Request, Snapshot,
};
use std::{
    fs,
    io::{self, Read, Write},
    net::Shutdown,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const MAX_CLIENTS: usize = 4;
const CLIENT_LIMIT: Duration = Duration::from_secs(1);
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid local session control peer/frame/path",
    )
}
fn uid() -> u32 {
    // SAFETY: get effective UID is a scalar query without pointers or side effects.
    unsafe { libc::geteuid() }
}
fn peer(stream: &UnixStream) -> io::Result<()> {
    let mut credential = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: initialized writable ucred and length have exact SDK size. The
    // borrowed stream FD stays live; getsockopt is synchronous and retains none.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credential as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>()
        || credential.uid != uid()
        || credential.pid <= 0
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}
fn path(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let name = path
        .file_name()
        .filter(|n| !n.is_empty())
        .ok_or_else(invalid)?;
    let parent = path.parent().ok_or_else(invalid)?.canonicalize()?;
    let metadata = fs::metadata(&parent)?;
    if !metadata.is_dir() || metadata.uid() != uid() || metadata.mode() & 0o077 != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let path = parent.join(name);
    if path.as_os_str().as_bytes().len() >= 108 || path.as_os_str().as_bytes().contains(&0) {
        return Err(invalid());
    }
    Ok(path)
}
struct Connection {
    stream: UnixStream,
    born: Instant,
    input: [u8; REQUEST_BYTES + 1],
    read: usize,
    response: Option<[u8; REPLY_BYTES]>,
    written: usize,
}
impl Connection {
    fn poll(&mut self, snapshot: Snapshot, stop: &mut bool) -> bool {
        if self.born.elapsed() >= CLIENT_LIMIT {
            return false;
        }
        if self.response.is_none() {
            match self.stream.read(&mut self.input[self.read..]) {
                Ok(0) => {
                    if self.read != REQUEST_BYTES {
                        return false;
                    }
                    let Ok(request) = Request::decode(&self.input[..self.read]) else {
                        return false;
                    };
                    let outcome = match request.command {
                        Command::Status => Outcome::Status,
                        Command::Stop { session } if session == snapshot.session => {
                            *stop = true;
                            Outcome::StopAccepted
                        }
                        Command::Stop { .. } => Outcome::StaleSession,
                    };
                    let mut snapshot = snapshot;
                    if *stop && !matches!(snapshot.phase, Phase::Exited | Phase::Unobserved) {
                        snapshot.phase = Phase::StopRequested;
                    }
                    let Ok(response) = Reply {
                        id: request.id,
                        outcome,
                        snapshot,
                    }
                    .encode() else {
                        return false;
                    };
                    self.response = Some(response);
                }
                Ok(count) => {
                    self.read += count;
                    if self.read > REQUEST_BYTES {
                        return false;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return false,
            }
        }
        if let Some(response) = self.response.as_ref() {
            match self.stream.write(&response[self.written..]) {
                Ok(0) => return false,
                Ok(count) => {
                    self.written += count;
                    return self.written < response.len();
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return false,
            }
        }
        true
    }
}
struct SocketPath {
    path: PathBuf,
    dev: u64,
    inode: u64,
}
pub struct Server {
    listener: Option<UnixListener>,
    owned: Option<SocketPath>,
    session: u64,
    clients: Vec<Connection>,
    stop: bool,
}
impl Server {
    /// Cold bind in an existing owner-only directory, never replaces any entry.
    /// Public nonce is generated independently from every private worker run ID.
    pub fn bind(socket: &Path) -> io::Result<Self> {
        Self::bind_id(
            socket,
            crate::run_identity::RunIdentity::generate()?.run_id(),
        )
    }
    fn bind_id(socket: &Path, session: u64) -> io::Result<Self> {
        if session == 0 {
            return Err(invalid());
        }
        let path = path(socket)?;
        let listener = UnixListener::bind(&path)?;
        let meta = fs::symlink_metadata(&path)?;
        let mut server = Self {
            listener: Some(listener),
            owned: Some(SocketPath {
                path: path.clone(),
                dev: meta.dev(),
                inode: meta.ino(),
            }),
            session,
            clients: Vec::new(),
            stop: false,
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        server
            .listener
            .as_mut()
            .ok_or_else(invalid)?
            .set_nonblocking(true)?;
        Ok(server)
    }
    pub fn session(&self) -> u64 {
        self.session
    }
    pub fn stop_requested(&self) -> bool {
        self.stop
    }
    /// Call after every supervisor iteration. Client errors close only their
    /// connection; listener errors are returned with the sticky stop flag retained.
    pub fn poll(&mut self, snapshot: Snapshot) -> io::Result<()> {
        snapshot.validate().map_err(|_| invalid())?;
        if snapshot.session != self.session {
            return Err(invalid());
        }
        let listener = self.listener.as_ref().ok_or(io::ErrorKind::NotConnected)?;
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(true)?;
                if self.clients.len() < MAX_CLIENTS && peer(&stream).is_ok() {
                    self.clients.push(Connection {
                        stream,
                        born: Instant::now(),
                        input: [0; REQUEST_BYTES + 1],
                        read: 0,
                        response: None,
                        written: 0,
                    });
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
        self.clients
            .retain_mut(|c| c.poll(snapshot, &mut self.stop));
        Ok(())
    }
    pub fn close(&mut self) -> io::Result<()> {
        self.clients.clear();
        self.listener.take();
        if let Some(owned) = self.owned.as_ref() {
            match fs::symlink_metadata(&owned.path) {
                Ok(meta)
                    if meta.dev() == owned.dev
                        && meta.ino() == owned.inode
                        && meta.file_type().is_socket() =>
                {
                    fs::remove_file(&owned.path)?
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "control socket path was replaced; retained foreign entry",
                    ));
                }
                Err(e) => return Err(e),
            }
        }
        self.owned = None;
        Ok(())
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
fn address(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() >= 108 || bytes.contains(&0) {
        return Err(invalid());
    }
    let mut address = libc::sockaddr_un {
        sun_family: libc::AF_UNIX as libc::sa_family_t,
        sun_path: [0; 108],
    };
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    Ok((
        address,
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t,
    ))
}
pub struct Client {
    stream: Option<UnixStream>,
    address: libc::sockaddr_un,
    length: libc::socklen_t,
    request: Request,
    encoded: [u8; REQUEST_BYTES],
    written: usize,
    input: [u8; REPLY_BYTES + 1],
    read: usize,
    connected: bool,
    half_closed: bool,
    deadline: Instant,
    result: Option<Reply>,
}
impl Client {
    pub fn connect(socket: &Path, request: Request, timeout: Duration) -> io::Result<Self> {
        if timeout.is_zero() || timeout > Duration::from_secs(5) {
            return Err(invalid());
        }
        let path = path(socket)?;
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_socket() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let (address, length) = address(&path)?;
        let encoded = request.encode().map_err(|_| invalid())?;
        // SAFETY: scalar-only socket call; immediately own its unique returned FD.
        let fd = unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: socket returned one fresh owned FD; no alias/duplicate is made.
        let stream = UnixStream::from(unsafe { OwnedFd::from_raw_fd(fd) });
        Ok(Self {
            stream: Some(stream),
            address,
            length,
            request,
            encoded,
            written: 0,
            input: [0; REPLY_BYTES + 1],
            read: 0,
            connected: false,
            half_closed: false,
            deadline: Instant::now().checked_add(timeout).ok_or_else(invalid)?,
            result: None,
        })
    }
    pub fn poll(&mut self) -> io::Result<Option<Reply>> {
        let result = self.poll_inner();
        if result.is_err() {
            self.stream = None;
        }
        result
    }
    fn poll_inner(&mut self) -> io::Result<Option<Reply>> {
        if let Some(result) = self.result {
            return Ok(Some(result));
        }
        if Instant::now() >= self.deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let stream = self.stream.as_mut().ok_or(io::ErrorKind::NotConnected)?;
        if !self.connected {
            // SAFETY: borrowed live FD, initialized native sockaddr_un, exact
            // bounded length including nul. NONBLOCK prevents connect waits.
            let result = unsafe {
                libc::connect(
                    stream.as_raw_fd(),
                    (&self.address as *const libc::sockaddr_un).cast(),
                    self.length,
                )
            };
            if result != 0 {
                let e = io::Error::last_os_error();
                match e.raw_os_error() {
                    Some(libc::EISCONN) => {}
                    Some(libc::EAGAIN | libc::EINPROGRESS | libc::EALREADY | libc::EINTR) => {
                        return Ok(None);
                    }
                    _ => return Err(e),
                }
            }
            peer(stream)?;
            self.connected = true;
        }
        if self.written < REQUEST_BYTES {
            match stream.write(&self.encoded[self.written..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => self.written += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
        if self.written == REQUEST_BYTES && !self.half_closed {
            stream.shutdown(Shutdown::Write)?;
            self.half_closed = true;
        }
        if !self.half_closed {
            return Ok(None);
        }
        match stream.read(&mut self.input[self.read..]) {
            Ok(0) => {
                let reply = Reply::decode(&self.input[..self.read]).map_err(|_| invalid())?;
                if !reply.matches(self.request) {
                    return Err(invalid());
                }
                self.result = Some(reply);
                self.stream = None;
                Ok(Some(reply))
            }
            Ok(n) => {
                self.read += n;
                if self.read > REPLY_BYTES {
                    return Err(invalid());
                }
                Ok(None)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::DirBuilderExt,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(1);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "kw-control-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn snapshot(server: &Server) -> Snapshot {
        Snapshot {
            session: server.session(),
            phase: Phase::Active,
            progress_epoch: 4,
            generation: Some(1),
            exit: None,
            recovery_confirmed: false,
            fault: false,
        }
    }
    fn exchange(server: &mut Server, request: Request) -> Reply {
        let path = server.owned.as_ref().unwrap().path.clone();
        let mut client = Client::connect(&path, request, Duration::from_secs(2)).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            server.poll(snapshot(server)).unwrap();
            if let Some(reply) = client.poll().unwrap() {
                return reply;
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
    #[test]
    #[ignore = "requires an environment permitting real AF_UNIX sockets; run explicitly with --ignored"]
    fn real_socket_status_and_nonce_fenced_stop_never_claim_exit_or_recovery() {
        let dir = Directory::new();
        let path = dir.0.join("control");
        let mut server = Server::bind_id(&path, 123).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        let reply = exchange(
            &mut server,
            Request {
                id: 1,
                command: Command::Status,
            },
        );
        assert_eq!(reply.snapshot.phase, Phase::Active);
        assert!(!server.stop_requested());
        let reply = exchange(
            &mut server,
            Request {
                id: 2,
                command: Command::Stop { session: 122 },
            },
        );
        assert_eq!(reply.outcome, Outcome::StaleSession);
        assert!(!server.stop_requested());
        let reply = exchange(
            &mut server,
            Request {
                id: 3,
                command: Command::Stop { session: 123 },
            },
        );
        assert_eq!(reply.outcome, Outcome::StopAccepted);
        assert!(server.stop_requested());
        assert_eq!(reply.snapshot.phase, Phase::StopRequested);
        assert_eq!(reply.snapshot.exit, None);
        assert!(!reply.snapshot.recovery_confirmed);
        server.close().unwrap();
        assert!(!path.exists());
    }
    #[test]
    #[ignore = "requires an environment permitting real AF_UNIX sockets; run explicitly with --ignored"]
    fn partial_extra_and_expired_requests_cannot_stop_or_hold_unbounded_slots() {
        let dir = Directory::new();
        let path = dir.0.join("control");
        let mut server = Server::bind_id(&path, 123).unwrap();
        let mut stream = UnixStream::connect(&path).unwrap();
        let frame = Request {
            id: 1,
            command: Command::Stop { session: 123 },
        }
        .encode()
        .unwrap();
        stream.write_all(&frame[..10]).unwrap();
        server.poll(snapshot(&server)).unwrap();
        assert!(!server.stop_requested());
        stream.write_all(&frame[10..]).unwrap();
        stream.write_all(&[0]).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        for _ in 0..4 {
            server.poll(snapshot(&server)).unwrap();
        }
        assert!(!server.stop_requested());
        assert!(server.clients.is_empty());
        let _slow = UnixStream::connect(&path).unwrap();
        server.poll(snapshot(&server)).unwrap();
        assert_eq!(server.clients.len(), 1);
        server.clients[0].born = Instant::now() - Duration::from_secs(2);
        server.poll(snapshot(&server)).unwrap();
        assert!(server.clients.is_empty());
        let mut client = Client::connect(
            &path,
            Request {
                id: 2,
                command: Command::Status,
            },
            Duration::from_secs(1),
        )
        .unwrap();
        client.deadline = Instant::now() - Duration::from_millis(1);
        assert_eq!(client.poll().unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(client.stream.is_none());
    }
    #[test]
    #[ignore = "requires an environment permitting real AF_UNIX sockets; run explicitly with --ignored"]
    fn existing_entries_shared_directories_and_replaced_owned_socket_are_preserved() {
        let dir = Directory::new();
        let path = dir.0.join("control");
        fs::write(&path, b"existing").unwrap();
        assert!(Server::bind_id(&path, 1).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"existing");
        fs::remove_file(&path).unwrap();
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Server::bind_id(&path, 1).is_err());
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o700)).unwrap();
        let mut server = Server::bind_id(&path, 1).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"replacement").unwrap();
        assert!(server.close().is_err());
        drop(server);
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
    }
}
