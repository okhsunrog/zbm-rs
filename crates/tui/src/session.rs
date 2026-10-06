//! Private one-byte datagram protocol. Child lifetime, not socket EOF, drives PID 1.
use std::{
    io,
    os::{fd::FromRawFd, unix::net::UnixDatagram},
};

#[cfg(any(feature = "vm-test", test))]
use std::os::fd::AsRawFd;

pub const FD_ENV: &str = "ZBM_SUPERVISOR_FD";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Request {
    Restart = 1,
    EmergencyShell = 2,
    Reboot = 3,
    PowerOff = 4,
    KexecStarting = 5,
}

impl TryFrom<u8> for Request {
    type Error = io::Error;
    fn try_from(value: u8) -> io::Result<Self> {
        match value {
            1 => Ok(Self::Restart),
            2 => Ok(Self::EmergencyShell),
            3 => Ok(Self::Reboot),
            4 => Ok(Self::PowerOff),
            5 => Ok(Self::KexecStarting),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unknown supervisor request",
            )),
        }
    }
}

pub struct Client(UnixDatagram);
impl Client {
    pub fn from_environment() -> io::Result<Option<Self>> {
        let Some(value) = std::env::var_os(FD_ENV) else {
            return Ok(None);
        };
        let fd: i32 = value
            .to_str()
            .and_then(|s| s.parse().ok())
            .filter(|&fd| fd >= 3)
            .ok_or_else(|| io::Error::other("Invalid supervisor descriptor"))?;
        // SAFETY: single-threaded entry, before any runtime or subprocess is started.
        unsafe {
            std::env::remove_var(FD_ENV);
        }
        // SAFETY: validate the inherited descriptor before taking ownership.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut kind = 0;
        let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
        // SAFETY: valid aligned output buffers for SO_TYPE.
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&mut kind as *mut i32).cast(),
                &mut length,
            )
        } < 0
            || kind != libc::SOCK_DGRAM
        {
            return Err(io::Error::other(
                "Supervisor descriptor is not a datagram socket",
            ));
        }
        // SAFETY: this process owns the descriptor passed only by its supervisor.
        let socket = unsafe { UnixDatagram::from_raw_fd(fd) };
        socket.set_write_timeout(Some(std::time::Duration::from_secs(1)))?;
        Ok(Some(Self(socket)))
    }
    pub fn request(&self, request: Request) -> io::Result<()> {
        if self.0.send(&[request as u8])? != 1 {
            return Err(io::Error::other("Short supervisor message"));
        }
        Ok(())
    }
    #[cfg(feature = "vm-test")]
    pub fn fd(&self) -> i32 {
        self.0.as_raw_fd()
    }
}

pub fn receive(socket: &UnixDatagram) -> io::Result<Option<Request>> {
    let mut bytes = [0u8; 2];
    match socket.recv(&mut bytes) {
        Ok(1) => Ok(Some(Request::try_from(bytes[0])?)),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid supervisor packet length",
        )),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn cloexec(fd: i32, enabled: bool) -> io::Result<()> {
    // SAFETY: fcntl only operates on the supplied descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let flags = if enabled {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn datagrams_preserve_boundaries_and_reject_unknown_or_oversized_messages() {
        let (a, b) = UnixDatagram::pair().unwrap();
        b.set_nonblocking(true).unwrap();
        a.send(&[Request::PowerOff as u8]).unwrap();
        assert_eq!(receive(&b).unwrap(), Some(Request::PowerOff));
        assert_eq!(receive(&b).unwrap(), None);
        a.send(&[99]).unwrap();
        assert!(receive(&b).is_err());
        a.send(&[1, 2, 3]).unwrap();
        assert!(receive(&b).is_err());
        assert_ne!(
            unsafe { libc::fcntl(a.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
}
