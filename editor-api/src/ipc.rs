use bevy::log::*;
use core::marker::PhantomData;
use std::os::unix::net::UnixDatagram;

use serde::{Serialize, de::DeserializeOwned};

pub struct IpcChannel<S, R> {
    _phantom: PhantomData<(S, R)>,
    buffer: Vec<u8>,
    socket: UnixDatagram,
    drop_socket: bool,
}

impl<S, R> Default for IpcChannel<S, R> {
    fn default() -> Self {
        let (socket, drop_socket) = match UnixDatagram::bind("/tmp/bevy.sock") {
            Ok(s) => (s, true),
            Err(err) => {
                let std::io::ErrorKind::AddrInUse = err.kind() else {
                    panic!("{}", err);
                };
                let socket = UnixDatagram::unbound().unwrap();
                socket.connect("/tmp/bevy.sock").unwrap();
                (socket, false)
            }
        };
        socket.set_nonblocking(true).unwrap();
        let buffer = vec![0; 8 * 1024];
        Self {
            _phantom: Default::default(),
            buffer,
            drop_socket,
            socket,
        }
    }
}

impl<S, R> Drop for IpcChannel<S, R> {
    fn drop(&mut self) {
        if self.drop_socket {
            _ = self.socket.shutdown(std::net::Shutdown::Both);
            _ = std::fs::remove_file("/tmp/bevy.sock");
        }
    }
}

impl<S: Serialize + core::fmt::Debug, R: DeserializeOwned> IpcChannel<S, R> {
    pub fn send(&mut self, msg: S) {
        let bytes = postcard::to_slice(&msg, &mut self.buffer).unwrap();
        info!("Sending: {msg:?}");
        _ = self.socket.send(&bytes);
    }

    pub fn recv(&mut self) -> Option<R> {
        let bytes_read = match self.socket.recv(&mut self.buffer) {
            Ok(s) => s,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    return None;
                } else {
                    panic!("{e}")
                }
            }
        };
        let msg = postcard::from_bytes(&mut self.buffer[..bytes_read]).unwrap();
        Some(msg)
    }
}
