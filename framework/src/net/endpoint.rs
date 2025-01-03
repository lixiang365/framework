//! socket
//!
//!
use mio::event::Source;
use mio::{Interest, Registry, Token};
use std::io;
use std::net::SocketAddr;

// 重新导出
pub use mio::net::TcpListener;
pub use mio::net::TcpStream;

use crate::reactor;
// 保存socket状态，方法封装

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum EndpointType {
    TCPListen,
    TCPClient,
}

// #[derive(Clone)]
pub enum SocketWrap {
    TCPListen(TcpListener),
    TCPClient(TcpStream),
}

pub struct Endpoint {
    // 地址
    pub addr: SocketAddr,
    pub socket: Option<TcpStream>,
    pub socket_wrap: Option<SocketWrap>,
    pub socket_listen: Option<TcpListener>,
    pub ep_type: EndpointType,
    pub token: usize,
}

impl Endpoint {
    pub fn new_tcp_listen(
        socket: TcpListener,
        addr: SocketAddr,
        token: usize,
    ) -> io::Result<Self> {
        // 注册事件
        // let registry = &reactor::Reactor::get().registry;
        // socket.register(registry, Token(token), Interest::READABLE)?;

        Ok(Self {
            addr: addr,
            socket: None,
            socket_listen: Some(socket),
            socket_wrap: None,
            ep_type: EndpointType::TCPListen,
            token,
        })
    }

    pub fn new_tcp_client(
        socket: TcpStream,
        addr: SocketAddr,
        token: usize,
    ) -> io::Result<Self> {
        // 注册事件
        // let registry = &reactor::Reactor::get().registry;
        // socket.register(
        //     registry,
        //     Token(token),
        //     Interest::READABLE.add(Interest::WRITABLE),
        // )?;
        Ok(Self {
            addr: addr,
            socket: Some(socket),
            socket_listen: None,
            socket_wrap: None,
            ep_type: EndpointType::TCPClient,
            token,
        })
    }

    pub fn register(&mut self) -> io::Result<()> {
        match self.ep_type {
            EndpointType::TCPListen => {
                // 注册事件
                let registry = &reactor::Reactor::get().registry;
                if let Some(mut socket) = self.socket_listen.take() {
                    socket.register(registry, Token(self.token), Interest::READABLE)?;
                    self.socket_listen = Some(socket);
                } else {
                    return Err(std::io::Error::other("not init"));
                }
            }
            EndpointType::TCPClient => {
                let registry = &reactor::Reactor::get().registry;
                if let Some(mut socket) = self.socket.take() {
                    socket.register(
                        registry,
                        Token(self.token),
                        Interest::READABLE.add(Interest::WRITABLE),
                    )?;
                    self.socket = Some(socket);
                } else {
                    return Err(std::io::Error::other("not init"));
                }
            }
        }
        Ok(())
    }

    pub fn is_connected_as_client(&self) -> io::Result<bool> {
        if self.ep_type == EndpointType::TCPListen {
            return Ok(true);
        }
        // If we hit an error while connecting return that error.
        if let Ok(Some(err)) | Err(err) = self.socket.as_ref().unwrap().take_error() {
            tracing::error!("is_connected_as_client - err:{:?}", err);
            return Err(err);
        }

        // If we can get a peer address it means the stream is
        // connected.
        match self.socket.as_ref().unwrap().peer_addr() {
            Ok(addr) => {
                // #[allow(unused_mut)]
                // let mut stream = TcpStream { socket };
                // #[cfg(target_os = "linux")]
                // if let Some(cpu) = self.cpu_affinity {
                //     if let Err(err) = stream.set_cpu_affinity(cpu) {
                //         warn!("failed to set CPU affinity on TcpStream: {}", err);
                //     }
                // }
                tracing::error!("is_connected_as_client - addr:{:?}", addr);
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
        match self.ep_type {
            EndpointType::TCPListen => {
                let registry = &reactor::Reactor::get().registry;
                if let Some(mut socket) = self.socket_listen.take() {
                    let _ = socket.deregister(registry);
                }
            }
            EndpointType::TCPClient => {
                let registry = &reactor::Reactor::get().registry;
                if let Some(mut socket) = self.socket.take() {
                    tracing::trace!("Endpint drop - deregister addr:{:?}", socket.peer_addr());
                    let _ = socket.deregister(registry);
                }
            }
        }
    }
}

impl Source for Endpoint {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        // Delegate the `register` call to `socket`
        if let Some(socket) = &mut self.socket_wrap {
            match socket {
                SocketWrap::TCPListen(tcp_listener) => {
                    tcp_listener.register(registry, token, interests)?;
                }
                SocketWrap::TCPClient(tcp_stream) => {
                    tcp_stream.register(registry, token, interests)?;
                }
            }
        }
        Ok(())
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        // Delegate the `reregister` call to `socket`
        if let Some(socket) = &mut self.socket_wrap {
            match socket {
                SocketWrap::TCPListen(tcp_listener) => {
                    tcp_listener.reregister(registry, token, interests)?;
                }
                SocketWrap::TCPClient(tcp_stream) => {
                    tcp_stream.reregister(registry, token, interests)?;
                }
            }
        }
        Ok(())
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        // Delegate the `deregister` call to `socket`
        if let Some(socket) = &mut self.socket_wrap {
            match socket {
                SocketWrap::TCPListen(tcp_listener) => {
                    tcp_listener.deregister(registry)?;
                }
                SocketWrap::TCPClient(tcp_stream) => {
                    tcp_stream.deregister(registry)?;
                }
            }
        }
        Ok(())
    }
}
