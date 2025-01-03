//! 消息过滤限流等抽象socket 通道
//!
//!

use std::{
    io::{self, Read, Write},
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        RwLock,
    },
};

use bytes::BufMut;
use mio::event::Event;
use tokio_util::codec::Decoder;

use super::endpoint::{Endpoint, EndpointType};
use crate::{
    net::message::{self, MessageItem},
    reactor::{self, Reactor},
};
pub use std::net::Shutdown;

// 忽略读写锁中毒
macro_rules! rwlock {
    ($lock_result:expr) => {{
        $lock_result.unwrap_or_else(|e| e.into_inner())
    }};
}

pub(crate) const CH_STATUS_DEFAULT: u32 = 0b00000000; // 初始化
pub(crate) const CH_STATUS_CONNECTED: u32 = 0b00000001; // 连接
pub(crate) const CH_STATUS_DESTROYED: u32 = 0b00000010; // 通道已经销毁
pub(crate) const CH_STATUS_HANDSHAKE: u32 = 0b00000100; // 已经握手过
pub(crate) const CH_STATUS_CONDEMN_AND_WAIT_DESTROY: u32 = 0b00001000; // 该频道已经变得不合法，即将在数据发送完毕后关闭
pub(crate) const CH_STATUS_CONDEMN_AND_DESTROY: u32 = 0b00010000; // 该频道已经变得不合法，即将关闭
pub(crate) const CH_STATUS_CONDEMN: u32 =
    CH_STATUS_CONDEMN_AND_WAIT_DESTROY | CH_STATUS_CONDEMN_AND_DESTROY;

pub struct Channel {
    pub(crate) endpoint: Endpoint,
    pub ep_type: EndpointType,
    pub channel_id: usize,
    // 服务器id，就是服务器监听通道的id，只有当这个通道是客户通道才有意义
    pub server_id: usize,
    // 发送缓冲区
    pub(crate) send_buf: RwLock<bytes::BytesMut>,
    // 是否可写
    pub(crate) is_write_able: AtomicBool,
    pub(crate) recv_buf: RwLock<bytes::BytesMut>,
    pub status: AtomicU32,
}

impl Channel {
    pub(crate) fn new(endpoint: Endpoint, channel_id: usize, server_id: usize) -> Self {
        let status = CH_STATUS_DEFAULT;
        Self {
            ep_type: endpoint.ep_type.clone(),
            endpoint: endpoint,
            channel_id,
            server_id,
            send_buf: RwLock::new(bytes::BytesMut::with_capacity(2048)),
            is_write_able: AtomicBool::new(true),
            recv_buf: RwLock::new(bytes::BytesMut::with_capacity(2048)),
            status: AtomicU32::new(status),
        }
    }

    // 初始化注册通道
    pub(crate) fn init(&mut self) -> io::Result<()> {
        self.endpoint.register()?;
        match self.ep_type {
            EndpointType::TCPListen => {
                self.status.store(CH_STATUS_CONNECTED, Ordering::Relaxed);
            }
            EndpointType::TCPClient => {}
        }
        Ok(())
    }

    // 接收新连接
    pub(crate) fn accept_new_conn(&self) -> io::Result<Vec<Channel>> {
        let mut new_channels = Vec::new();
        loop {
            let (connection, address) = match self.endpoint.socket_listen.as_ref().unwrap().accept()
            {
                Ok((connection, address)) => (connection, address),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // If we get a `WouldBlock` error we know our
                    // listener has no more incoming connections queued,
                    // so we can return to polling and wait for some
                    // more.
                    break;
                }
                Err(e) => {
                    // If it was any other kind of error, something went
                    // wrong and we terminate with an error.
                    return Err(e);
                }
            };
            let token = reactor::Reactor::get().next_token();
            let new_ep = Endpoint::new_tcp_client(connection, address, token)?;
            let new_ch = Channel::new(new_ep, token, self.channel_id);
            new_ch.status.store(CH_STATUS_CONNECTED, Ordering::Relaxed);
            new_channels.push(new_ch);
            println!(
                "Accepted connection from: {} | channel_id:{}",
                address, token
            );
        }
        Ok(new_channels)
    }

    // 通道事件处理
    pub(crate) fn handle_connection_event(&self, event: &Event) -> io::Result<()> {
        if self.ep_type == EndpointType::TCPClient {
            if event.is_writable() {
                self.is_write_able.store(true, Ordering::Relaxed);
                println!(
                    "handle_connection_event - status: {:?}",
                    self.status.load(Ordering::Relaxed)
                );
                if self.status.load(Ordering::Relaxed) == CH_STATUS_DEFAULT {
                    if let Some(connected) = self.check_connected_to_sever_as_client() {
                        // 如果连接失败就直接返回
                        if !connected {
                            return Err(std::io::Error::other("connect failed"));
                        }
                    }
                }
                loop {
                    let mut send_buf = rwlock!(self.send_buf.write());
                    if !send_buf.is_empty() {
                        let buf = &send_buf;
                        // We can (maybe) write to the connection.
                        match self.endpoint.socket.as_ref().unwrap().write(buf) {
                            // We want to write the entire `DATA` buffer in a single go. If we
                            // write less we'll return a short write error (same as
                            // `io::Write::write_all` does).
                            Ok(n) if n < buf.len() => {
                                self.is_write_able.store(false, Ordering::Relaxed);
                                let _ = send_buf.split_to(n);
                                println!("Write size: {:?}", n);
                                //
                                // return Err(io::ErrorKind::WriteZero.into());
                                break;
                            }
                            Ok(n) => {
                                let _ = send_buf.split_to(n);
                                // After we've written something we'll reregister the connection
                                // to only respond to readable events.
                                //
                                // registry.reregister(connection, event.token(), Interest::READABLE)?
                                println!("Write done size: {:?}", n);
                                break;
                            }

                            // Would block "errors" are the OS's way of saying that the
                            // connection is not actually ready to perform this I/O operation.
                            Err(ref err) if would_block(err) => {
                                break;
                            }
                            // Got interrupted (how rude!), we'll try again.
                            Err(ref err) if interrupted(err) => {
                                continue;
                            }
                            // Other errors we'll consider fatal.
                            Err(err) => {
                                return {
                                    println!("read err : {}", err);
                                    Err(err)
                                }
                            }
                        }
                    } else {
                        break;
                    }
                }
            }

            if event.is_readable() {
                let mut connection_closed = false;
                let mut received_data = vec![0; 4096];
                let mut bytes_read = 0;
                // We can (maybe) read from the connection.
                loop {
                    match self
                        .endpoint
                        .socket
                        .as_ref()
                        .unwrap()
                        .read(&mut received_data[bytes_read..])
                    {
                        Ok(0) => {
                            // Reading 0 bytes means the other side has closed the
                            // connection or is done writing, then so are we.
                            connection_closed = true;
                            break;
                        }
                        Ok(n) => {
                            bytes_read += n;
                            if bytes_read == received_data.len() {
                                received_data.resize(received_data.len() + 1024, 0);
                            }
                        }
                        // Would block "errors" are the OS's way of saying that the
                        // connection is not actually ready to perform this I/O operation.
                        Err(ref err) if would_block(err) => break,
                        Err(ref err) if interrupted(err) => continue,
                        // Other errors we'll consider fatal.
                        Err(err) => {
                            return {
                                println!("read err : {}", err);
                                Err(err)
                            }
                        }
                    }
                }

                if bytes_read != 0 {
                    // println!("read size: {}", bytes_read);
                    let mut recv_buf = rwlock!(self.recv_buf.write());
                    recv_buf.put_slice(&received_data[..bytes_read]);
                    let mut decoder = message::MessagePacketCodec {};
                    // 解码
                    loop {
                        match decoder.decode(&mut recv_buf) {
                            Ok(msg_opt) => {
                                if let Some(msg) = msg_opt {
                                    let _ = reactor::Reactor::get().recv_msg_channel_s.send(
                                        MessageItem {
                                            channel_id: self.channel_id,
                                            server_id: self.server_id,
                                            msg: msg,
                                        },
                                    );
                                    if recv_buf.len() >= 8 {
                                        continue;
                                    } else {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }
                            Err(e) => {
                                print!("handle_connection_event - decode byte err: {}", e);
                                return Err(e);
                            }
                        }
                    }
                }
                if connection_closed {
                    println!("Connection closed");
                    return Err(std::io::Error::other("con closed"));
                }
            }
        }
        Ok(())
    }

    // 检查作为客户端是否连接到服务器
    // 返回通道是否确定连接结果
    // 因为可能连接还没结果
    fn check_connected_to_sever_as_client(&self) -> Option<bool> {
        match self.endpoint.is_connected_as_client() {
            Ok(true) => {
                self.status.store(CH_STATUS_CONNECTED, Ordering::Relaxed);
                // 通知上层
                let msg = MessageItem {
                    server_id: self.server_id,
                    channel_id: self.channel_id,
                    msg: message::MessagePacket {
                        msg_id: message::FixedMessageId::Connected.as_i32(),
                        buf: vec![],
                    },
                };
                let _ = Reactor::get().recv_msg_channel_s.send(msg);
                println!(
                    "check_connected_to_sever_as_client - is connected ok(true) id: {:?}",
                    self.channel_id
                );
                return Some(true);
            }
            Ok(false) => {
                println!(
                    "check_connected_to_sever_as_client - is connected  ok(false)  id: {:?}",
                    self.channel_id
                );
                return None;
            }
            Err(_) => {
                return Some(false);
            }
        }
    }

    pub(crate) fn send_data(&self, buf: &[u8]) -> io::Result<()> {
        if self.ep_type == EndpointType::TCPClient {
            if self.is_write_able.load(Ordering::Relaxed) == true {
                loop {
                    if !buf.is_empty() {
                        // We can (maybe) write to the connection.
                        match self.endpoint.socket.as_ref().unwrap().write(buf) {
                            // We want to write the entire `DATA` buffer in a single go. If we
                            // write less we'll return a short write error (same as
                            // `io::Write::write_all` does).
                            Ok(n) if n < buf.len() => {
                                self.is_write_able.store(false, Ordering::Relaxed);
                                // 把没写完的存到buffer里
                                let (_, wait_send) = buf.split_at(n);
                                let mut send_buf = rwlock!(self.send_buf.write());
                                send_buf.put_slice(wait_send);
                                println!("Write size: {:?}", n);
                                //
                                // return Err(io::ErrorKind::WriteZero.into());
                                break;
                            }
                            Ok(n) => {
                                // After we've written something we'll reregister the connection
                                // to only respond to readable events.
                                //
                                // registry.reregister(connection, event.token(), Interest::READABLE)?
                                println!("Write done size: {:?}", n);
                                break;
                            }

                            // Would block "errors" are the OS's way of saying that the
                            // connection is not actually ready to perform this I/O operation.
                            Err(ref err) if would_block(err) => {
                                break;
                            }
                            // Got interrupted (how rude!), we'll try again.
                            Err(ref err) if interrupted(err) => {
                                continue;
                            }
                            // Other errors we'll consider fatal.
                            Err(err) => return Err(err),
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    // 关闭连接
    pub(crate) fn close_connect(&self, how: Shutdown) -> io::Result<()> {
        match self.ep_type {
            EndpointType::TCPListen => {
                return Ok(());
            }
            EndpointType::TCPClient => {
                self.endpoint.socket.as_ref().unwrap().shutdown(how)?;
            }
        }
        Ok(())
    }

    // 获取本地通道地址
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match self.ep_type {
            EndpointType::TCPListen => {
                if self.endpoint.socket_listen.is_some() {
                    return self.endpoint.socket_listen.as_ref().unwrap().local_addr();
                } else {
                    Err(std::io::Error::other("Registering"))
                }
            }
            EndpointType::TCPClient => {
                if self.endpoint.socket.is_some() {
                    return self.endpoint.socket.as_ref().unwrap().local_addr();
                } else {
                    Err(std::io::Error::other("Registering"))
                }
            }
        }
    }

    // 获取对端通道地址
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        match self.ep_type {
            EndpointType::TCPListen => Err(std::io::Error::other("ListenNotPeer")),
            EndpointType::TCPClient => {
                if self.endpoint.socket.is_some() {
                    return self.endpoint.socket.as_ref().unwrap().peer_addr();
                } else {
                    Err(std::io::Error::other("Registering"))
                }
            }
        }
    }
}

fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}

fn interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
}
