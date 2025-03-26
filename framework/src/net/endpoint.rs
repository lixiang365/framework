//! socket
//!
//! 保存socket状态，方法封装
use mio::event::Source;
use mio::{Interest, Token};
use std::io;
use std::net::SocketAddr;

use crate::util::static_mut_ref;
// 重新导出
use crate::REGISTRY;
pub use mio::net::TcpListener;
pub use mio::net::TcpStream;
pub use mio::net::UdpSocket;

pub struct BorrowedUDPSocket {
    pub source_id: usize,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum EndpointType {
    TCPListen,
    TCPStream,
    UDPSocket,
}

// #[derive(Clone)]
pub enum SocketWrap {
    TCPListen(TcpListener),
    TCPStream(TcpStream),
    UDPSocket(UdpSocket, std::net::UdpSocket),
    BorrowedUDPSocket(UdpSocket),
}

impl SocketWrap {
    pub fn get_tcp_listener(&self) -> Option<&TcpListener> {
        match self {
            SocketWrap::TCPListen(listener) => Some(listener),
            SocketWrap::TCPStream(_) => None,
            SocketWrap::UDPSocket(_, _) => None,
            SocketWrap::BorrowedUDPSocket(_) => None,
        }
    }

    pub fn get_tcp_stream(&self) -> Option<&TcpStream> {
        match self {
            SocketWrap::TCPListen(_) => None,
            SocketWrap::TCPStream(stream) => Some(stream),
            SocketWrap::UDPSocket(_, _) => None,
            SocketWrap::BorrowedUDPSocket(_) => None,
        }
    }

    pub fn get_udp_socket(&self) -> Option<&UdpSocket> {
        match self {
            SocketWrap::TCPListen(_) => None,
            SocketWrap::TCPStream(_) => None,
            SocketWrap::UDPSocket(socket, _) => Some(socket),
            SocketWrap::BorrowedUDPSocket(_) => None,
        }
    }

    pub fn get_std_udp_socket(&self) -> Option<&std::net::UdpSocket> {
        match self {
            SocketWrap::TCPListen(_) => None,
            SocketWrap::TCPStream(_) => None,
            SocketWrap::UDPSocket(_, socket) => Some(socket),
            SocketWrap::BorrowedUDPSocket(_) => None,
        }
    }

    pub fn get_borrow_udp_socket(&self) -> Option<&UdpSocket> {
        match self {
            SocketWrap::TCPListen(_) => None,
            SocketWrap::TCPStream(_) => None,
            SocketWrap::UDPSocket(_, _) => None,
            SocketWrap::BorrowedUDPSocket(borrowed_udpsocket) => Some(borrowed_udpsocket),
        }
    }

    pub fn is_listener(&self) -> bool {
        matches!(*self, SocketWrap::TCPListen(_))
    }

    pub fn is_tcp_stream(&self) -> bool {
        matches!(*self, SocketWrap::TCPStream(_))
    }

    pub fn is_udp_socket(&self) -> bool {
        matches!(*self, SocketWrap::UDPSocket(_, _))
    }

    pub fn is_borrow_udp_socket(&self) -> bool {
        matches!(*self, SocketWrap::BorrowedUDPSocket(_))
    }
}

pub struct Endpoint {
    // 地址
    pub addr: SocketAddr,
    pub socket_wrap: SocketWrap,
    pub ep_type: EndpointType,
    pub token: usize,
}

impl Endpoint {
    pub fn new_tcp_listen(socket: TcpListener, addr: SocketAddr, token: usize) -> io::Result<Self> {
        Ok(Self {
            addr: addr,
            socket_wrap: SocketWrap::TCPListen(socket),
            ep_type: EndpointType::TCPListen,
            token,
        })
    }

    pub fn new_tcp_client(socket: TcpStream, addr: SocketAddr, token: usize) -> io::Result<Self> {
        Ok(Self {
            addr: addr,
            socket_wrap: SocketWrap::TCPStream(socket),
            ep_type: EndpointType::TCPStream,
            token,
        })
    }

    pub fn new_udp_socket(
        std_socket: std::net::UdpSocket,
        socket: UdpSocket,
        addr: SocketAddr,
        token: usize,
    ) -> io::Result<Self> {
        Ok(Self {
            addr: addr,
            socket_wrap: SocketWrap::UDPSocket(socket, std_socket),
            ep_type: EndpointType::UDPSocket,
            token,
        })
    }

    pub fn new_borrow_udp_socket(
        socket: UdpSocket,
        addr: SocketAddr,
        token: usize,
    ) -> io::Result<Self> {
        Ok(Self {
            addr: addr,
            socket_wrap: SocketWrap::BorrowedUDPSocket(socket),
            ep_type: EndpointType::UDPSocket,
            token,
        })
    }

    pub fn register(&mut self) -> io::Result<()> {
        match &mut self.socket_wrap {
            SocketWrap::TCPListen(socket) => {
                // 注册事件
                socket.register(
                    static_mut_ref!(REGISTRY).as_ref().unwrap(),
                    Token(self.token),
                    Interest::READABLE,
                )?;
            }
            SocketWrap::TCPStream(socket) => {
                socket.register(
                    static_mut_ref!(REGISTRY).as_ref().unwrap(),
                    Token(self.token),
                    Interest::READABLE.add(Interest::WRITABLE),
                )?;
            }
            SocketWrap::UDPSocket(socket, _) => {
                socket.register(
                    static_mut_ref!(REGISTRY).as_ref().unwrap(),
                    Token(self.token),
                    Interest::READABLE,
                )?;
            }
            SocketWrap::BorrowedUDPSocket(_) => {}
        }
        Ok(())
    }

    pub fn reregister(&mut self) -> io::Result<()> {
        match &mut self.socket_wrap {
            SocketWrap::TCPListen(socket) => {
                // 注册事件
                socket.reregister(
                    static_mut_ref!(REGISTRY).as_ref().unwrap(),
                    Token(self.token),
                    Interest::READABLE,
                )?;
            }
            SocketWrap::TCPStream(socket) => {
                socket.reregister(
                    static_mut_ref!(REGISTRY).as_ref().unwrap(),
                    Token(self.token),
                    Interest::READABLE.add(Interest::WRITABLE),
                )?;
            }
            SocketWrap::UDPSocket(socket, _) => {
                socket.reregister(
                    static_mut_ref!(REGISTRY).as_ref().unwrap(),
                    Token(self.token),
                    Interest::READABLE,
                )?;
            }
            SocketWrap::BorrowedUDPSocket(_) => {}
        }
        Ok(())
    }

    pub fn is_connected_as_client(&self) -> io::Result<bool> {
        if self.ep_type == EndpointType::TCPListen {
            return Ok(true);
        }
        // If we hit an error while connecting return that error.
        if let Ok(Some(err)) | Err(err) = self.socket_wrap.get_tcp_stream().unwrap().take_error() {
            tracing::error!("is_connected_as_client - err:{:?}", err);
            return Err(err);
        }
        // If we can get a peer address it means the stream is
        // connected.
        match self.socket_wrap.get_tcp_stream().unwrap().peer_addr() {
            Ok(addr) => {
                // #[allow(unused_mut)]
                // let mut stream = TcpStream { socket };
                // #[cfg(target_os = "linux")]
                // if let Some(cpu) = self.cpu_affinity {
                //     if let Err(err) = stream.set_cpu_affinity(cpu) {
                //         warn!("failed to set CPU affinity on TcpStream: {}", err);
                //     }
                // }
                tracing::trace!("is_connected_as_client - addr:{:?}", addr);
                Ok(true)
            }
            // `NotConnected` (`ENOTCONN`) means the socket not yet
            // connected, but still working on it. `ECONNREFUSED` will
            // be reported if it fails.
            Err(err)
                if err.kind() == io::ErrorKind::NotConnected
                    || err.raw_os_error() == Some(libc::EINPROGRESS) =>
            {
                // Socket is not (yet) connected but haven't hit an
                // error either. So we return `Pending` and wait for
                // another event.
                // self.socket = Some(socket);
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        match &mut self.socket_wrap {
            SocketWrap::TCPListen(socket) => {
                let _ = socket.deregister(static_mut_ref!(REGISTRY).as_ref().unwrap());
            }
            SocketWrap::TCPStream(socket) => {
                let _ = socket.deregister(static_mut_ref!(REGISTRY).as_ref().unwrap());
            }
            SocketWrap::UDPSocket(socket, _) => {
                let _ = socket.deregister(static_mut_ref!(REGISTRY).as_ref().unwrap());
            }
            SocketWrap::BorrowedUDPSocket(_) => {}
        }
    }
}
