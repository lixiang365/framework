//! 消息过滤限流等抽象socket 通道
//!
//!

use std::io::{self, Read, Write};
use std::net::SocketAddr;

use bitflags::bitflags;
use bytes::BufMut;
use mio::event::Event;
use tokio_util::codec::{Decoder, Encoder};

use crate::net::reactor;
use crate::net::{
    endpoint::{Endpoint, EndpointType},
    message::{self, MessageItem, MessagePacket},
};
use crate::RECV_MSG_CH_S;
pub use std::net::Shutdown;

use crate::util::static_mut_ref;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct FilterFlags: u32 {
        // 原始字节
        const RAW_BYTE =	0b00000001;
        // 打包消息
        const PACK =		0b00000010;
        // 打包消息，包含消息id
        const PACK_MSGID =  0b00000100;
        // 加密
        const ENCRYPT=      0b00001000;

        // 原字节加密
        const RAW_BYTE_ENCRYPT = Self::RAW_BYTE.bits()|Self::ENCRYPT.bits();
        // 打包加密
        const PACK_MSGID_AND_ENCRYPT = Self::PACK_MSGID.bits()|Self::ENCRYPT.bits();
    }
}

impl FilterFlags {
    fn clear(&mut self) {
        self.set(FilterFlags::RAW_BYTE, true);
    }
}

// 通道选项
pub struct ChannelOption {
    pub filter_flags: FilterFlags,
}

impl Default for ChannelOption {
    fn default() -> Self {
        Self {
            filter_flags: FilterFlags::PACK_MSGID,
        }
    }
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
    // 服务器id，就是服务器监听通道的id，
    // 当 channel_id == server_id 当前通道就是服务器监听通道。
    // 当 server_id == 0 当前通道是作为客户端连接到服务器的通道
    // 当 server_id != channel_id 当前通道是连接到服务器的客户端通道
    pub server_id: usize,
    // 发送缓冲区
    pub(crate) send_buf: bytes::BytesMut,
    // 是否可写
    pub(crate) is_write_able: bool,
    pub(crate) recv_buf: bytes::BytesMut,
    pub status: u32,
    // 过滤器标志
    pub filter_flags: FilterFlags,

    // AES加密
    aes256_key: String,
    aes256_iv: String,
}

impl Channel {
    pub(crate) fn new(
        channel_option: &ChannelOption,
        endpoint: Endpoint,
        channel_id: usize,
        server_id: usize,
    ) -> Self {
        let status = CH_STATUS_DEFAULT;
        Self {
            ep_type: endpoint.ep_type.clone(),
            endpoint: endpoint,
            channel_id,
            server_id,
            send_buf: bytes::BytesMut::with_capacity(2048),
            is_write_able: true,
            recv_buf: bytes::BytesMut::with_capacity(2048),
            status: status,
            filter_flags: channel_option.filter_flags,
            aes256_key: String::new(),
            aes256_iv: String::new(),
        }
    }

    pub(crate) fn set_aes_key(&mut self, key: String, iv: String) {
        self.aes256_key = key;
        self.aes256_iv = iv;
        if !self.aes256_key.is_empty() && !self.aes256_iv.is_empty() {
            self.filter_flags.insert(FilterFlags::ENCRYPT);
        } else {
            self.filter_flags.remove(FilterFlags::ENCRYPT);
        }
    }

    // 初始化注册通道
    pub(crate) fn init(&mut self) -> io::Result<()> {
        self.endpoint.register()?;
        match self.ep_type {
            EndpointType::TCPListen => {
                self.status = CH_STATUS_CONNECTED;
            }
            EndpointType::TCPStream => self.status = CH_STATUS_CONNECTED,
            EndpointType::UDPSocket => {
                self.status = CH_STATUS_CONNECTED;
            }
        }
        Ok(())
    }

    // 是否是服务器监听通道
    pub fn is_server_listen(&self) -> bool {
        return self.channel_id == self.server_id;
    }

    // 是否是服务器客户端通道
    pub fn is_server_client(&self) -> bool {
        return (self.server_id != 0) && (self.channel_id != self.server_id);
    }

    // 是否是作为客户端通道
    pub fn is_as_client(&self) -> bool {
        return self.server_id == 0;
    }

    // 接收新连接
    pub(crate) fn accept_new_conn(&self) -> io::Result<Vec<Channel>> {
        let mut new_channels = Vec::new();
        loop {
            let (connection, address) = match self
                .endpoint
                .socket_wrap
                .get_tcp_listener()
                .unwrap()
                .accept()
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
            let token = reactor::next_token();
            let new_ep = Endpoint::new_tcp_client(connection, address, token)?;
            let mut new_ch = Channel::new(
                &ChannelOption {
                    filter_flags: self.filter_flags,
                },
                new_ep,
                token,
                self.channel_id,
            );
            new_ch.status = CH_STATUS_CONNECTED;
            new_channels.push(new_ch);
            tracing::debug!(
                "Accepted connection from: {} | channel_id:{}",
                address,
                token
            );
        }
        Ok(new_channels)
    }

    // 通道事件处理
    pub(crate) fn handle_connection_event(&mut self, event: &Event) -> io::Result<()> {
        if self.ep_type == EndpointType::TCPStream {
            self.tcp_handle_event(event)?;
        } else if self.ep_type == EndpointType::UDPSocket {
            if self.is_as_client() {
                self.udp_client_handle_event(event)?;
            }
        }
        Ok(())
    }

    pub(crate) fn udp_listen_handle_event(
        &mut self,
        event: &Event,
    ) -> io::Result<(SocketAddr, Vec<u8>)> {
        if event.is_readable() {
            let mut received_data = vec![0; 65536];
            let mut bytes_read = 0;
            let mut peer_addr = None;
            // We can (maybe) read from the connection.
            loop {
                match self
                    .endpoint
                    .socket_wrap
                    .get_udp_socket()
                    .unwrap()
                    .recv_from(&mut received_data[bytes_read..])
                {
                    Ok((n, addr)) => {
                        bytes_read += n;
                        peer_addr = Some(addr);
                        break;
                    }
                    // Would block "errors" are the OS's way of saying that the
                    // connection is not actually ready to perform this I/O operation.
                    Err(ref err) if would_block(err) => break,
                    Err(ref err) if interrupted(err) => continue,
                    // Other errors we'll consider fatal.
                    Err(err) => {
                        return {
                            tracing::error!("read err : {}", err);
                            Err(err)
                        }
                    }
                }
            }
            let _ = self.endpoint.reregister();
            return Ok((
                peer_addr.as_ref().unwrap().clone(),
                received_data[..bytes_read].to_vec(),
            ));
        } else {
            return Err(std::io::Error::other("con closed"));
        }
    }

    pub(crate) fn udp_client_handle_event(&mut self, event: &Event) -> io::Result<()> {
        if event.is_readable() {
            // let mut connection_closed = false;
            let mut received_data = vec![0; 65536];
            let mut bytes_read = 0;
            // let mut peer_addr = None;
            // We can (maybe) read from the connection.
            loop {
                match self
                    .endpoint
                    .socket_wrap
                    .get_udp_socket()
                    .unwrap()
                    .recv_from(&mut received_data[bytes_read..])
                {
                    Ok((n, _)) => {
                        bytes_read += n;
                        if bytes_read == received_data.len() {
                            received_data.resize(received_data.len() + 1024, 0);
                        }
                        // peer_addr = Some(addr);
                        // break;
                    }
                    // Would block "errors" are the OS's way of saying that the
                    // connection is not actually ready to perform this I/O operation.
                    Err(ref err) if would_block(err) => break,
                    Err(ref err) if interrupted(err) => continue,
                    // Other errors we'll consider fatal.
                    Err(err) => {
                        return {
                            tracing::error!("read err : {}", err);
                            Err(err)
                        }
                    }
                }
            }
            let _ = self.endpoint.reregister();
            self.recv_buf.put_slice(&received_data[..bytes_read]);
            self.recv_msg_filter()?;
        }
        Ok(())
    }

    pub(crate) fn tcp_handle_event(&mut self, event: &Event) -> io::Result<()> {
        if event.is_writable() {
            self.is_write_able = true;
            loop {
                if !self.send_buf.is_empty() {
                    let buf = &self.send_buf;
                    // We can (maybe) write to the connection.
                    match self
                        .endpoint
                        .socket_wrap
                        .get_tcp_stream()
                        .unwrap()
                        .write(buf)
                    {
                        // We want to write the entire `DATA` buffer in a single go. If we
                        // write less we'll return a short write error (same as
                        // `io::Write::write_all` does).
                        Ok(n) if n < buf.len() => {
                            self.is_write_able = false;
                            let _ = self.send_buf.split_to(n);
                            tracing::trace!(
                                "channel_id: {} | Write size: {:?}",
                                self.channel_id,
                                n
                            );
                            //
                            // return Err(io::ErrorKind::WriteZero.into());
                            break;
                        }
                        Ok(n) => {
                            let _ = self.send_buf.split_to(n);
                            // After we've written something we'll reregister the connection
                            // to only respond to readable events.
                            //
                            // registry.reregister(connection, event.token(), Interest::READABLE)?
                            tracing::trace!(
                                "channel_id: {} | Write done size: {:?}",
                                self.channel_id,
                                n
                            );
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

        if event.is_readable() {
            let mut connection_closed = false;
            let mut received_data = vec![0; 4096];
            let mut bytes_read = 0;
            // We can (maybe) read from the connection.
            loop {
                match self
                    .endpoint
                    .socket_wrap
                    .get_tcp_stream()
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
                            tracing::error!("read err : {}", err);
                            Err(err)
                        }
                    }
                }
            }

            if bytes_read != 0 {
                tracing::debug!("read size: {}", bytes_read);
                self.recv_buf.put_slice(&received_data[..bytes_read]);
                let _ = self.recv_msg_filter();
            }
            if connection_closed {
                tracing::debug!("Connection closed");
                return Err(std::io::Error::other("ConClosed"));
            }
        }
        Ok(())
    }

    // 检查作为客户端是否连接到服务器
    // 返回通道是否确定连接结果
    // 因为可能连接还没结果
    fn check_connected_to_sever_as_client(&mut self) -> Option<bool> {
        match self.endpoint.is_connected_as_client() {
            Ok(true) => {
                self.status = CH_STATUS_CONNECTED;
                // 通知上层
                let msg = MessageItem {
                    server_id: self.server_id,
                    channel_id: self.channel_id,
                    msg: message::MessagePacket {
                        msg_id: message::FixedMessageId::Connected.as_i32(),
                        buf: vec![],
                    },
                };
                // let a =static_mut!(RECV_MSG_CH_S);
                let _ = static_mut_ref!(RECV_MSG_CH_S).as_ref().unwrap().send(msg);
                tracing::debug!(
                    "check_connected_to_sever_as_client - is connected ok(true) id: {:?}",
                    self.channel_id
                );
                return Some(true);
            }
            Ok(false) => {
                tracing::debug!(
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

    pub(crate) fn send_msg_filter(&mut self, msg: MessagePacket) -> io::Result<()> {
        let mut buf = bytes::BytesMut::new();
        // 1. 是否打包
        if self.filter_flags.contains(FilterFlags::PACK_MSGID) {
            // 封包消息id
            let mut encoder = message::MessagePacketCodec {};
            match encoder.encode(msg, &mut buf) {
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("send_msg - data too long err:{}", e);
                    return Err(e);
                }
            }
        } else if self.filter_flags.contains(FilterFlags::RAW_BYTE) {
            // 直接发送原始字节
            buf.put_slice(&msg.buf);
        } else if self.filter_flags.contains(FilterFlags::PACK) {
            // TODO 封包原始字节，解决tcp粘包
        }

        // 2.是否加密
        let mut dst_buf = bytes::BytesMut::new();
        if self.filter_flags.contains(FilterFlags::ENCRYPT) {
            let mut encrypt = message::MessagePacketEncryptCodec {};
            match encrypt.encode(&mut buf, &mut dst_buf, &self.aes256_key, &self.aes256_iv) {
                Ok(_) => {}
                Err(e) => {
                    tracing::error!("send_msg - encrypt err:{}", e);
                    return Err(e);
                }
            }
            buf = dst_buf;
        }
        self.send_data(&buf)
    }

    pub(crate) fn recv_msg_filter(&mut self) -> io::Result<()> {
        let src = &mut self.recv_buf;
        let decode_packet_data_ref;
        let mut decode_packet_data = bytes::BytesMut::new();
        // 1.是否加密
        if self.filter_flags.contains(FilterFlags::ENCRYPT) {
            let mut decrypt = message::MessagePacketEncryptCodec {};
            loop {
                match decrypt.decode(src, &self.aes256_key, &self.aes256_iv) {
                    Ok(Some(decrypt_data)) => {
                        decode_packet_data.extend(decrypt_data);
                        // src = &mut src_data;
                        if src.len() >= 8 {
                            continue;
                        } else {
                            break;
                        }
                    }
                    Ok(None) => {
                        tracing::debug!("recv_msg_filter - decrypt byte None");
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::error!("recv_msg_filter - decrypt err:{}", e);
                        return Ok(());
                    }
                }
            }
            decode_packet_data_ref = &mut decode_packet_data;
        } else {
            decode_packet_data_ref = src;
        }

        // 2. 是否解包
        let mut decoder = message::MessagePacketCodec {};
        if self.filter_flags.contains(FilterFlags::PACK_MSGID) {
            loop {
                match decoder.decode(decode_packet_data_ref) {
                    Ok(Some(msg)) => {
                        let _ =
                            static_mut_ref!(RECV_MSG_CH_S)
                                .as_ref()
                                .unwrap()
                                .send(MessageItem {
                                    channel_id: self.channel_id,
                                    server_id: self.server_id,
                                    msg: msg,
                                });
                        tracing::debug!("recv_msg_filter - decode packet");
                        if decode_packet_data_ref.len() >= 8 {
                            continue;
                        } else {
                            break;
                        }
                    }
                    Ok(None) => {
                        tracing::debug!("recv_msg_filter - decode byte None");
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::debug!("recv_msg_filter - decode byte err: {}", e);
                        return Err(e);
                    }
                }
            }
        } else if self.filter_flags.contains(FilterFlags::RAW_BYTE) {
            let _ = static_mut_ref!(RECV_MSG_CH_S)
                .as_ref()
                .unwrap()
                .send(MessageItem {
                    channel_id: self.channel_id,
                    server_id: self.server_id,
                    msg: MessagePacket {
                        msg_id: 0,
                        buf: self.recv_buf.split().to_vec(),
                    },
                });
        }
        Ok(())
    }

    pub(crate) fn send_data(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.status == CH_STATUS_CONNECTED {
            if self.ep_type == EndpointType::TCPStream {
                self.tcp_send_data(buf)?
            } else if self.ep_type == EndpointType::UDPSocket {
                if self.is_server_listen() || self.is_server_client() {
                    self.udp_send_data(buf)?
                } else {
                    self.udp_client_send_data(buf)?
                }
            }
        } else if self.status == CH_STATUS_DEFAULT {
            tracing::error!("send data not connected");
            return Err(std::io::Error::other("not connected"));
        } else {
            tracing::error!("send data not connected 2");
            return Err(std::io::Error::other("not connected 2"));
        }
        Ok(())
    }

    pub(crate) fn udp_send_data(&mut self, buf: &[u8]) -> io::Result<()> {
        // if self.is_write_able {
        loop {
            if !buf.is_empty() {
                // We can (maybe) write to the connection.
                let peer_addr = self.endpoint.addr;

                match self
                    .endpoint
                    .socket_wrap
                    .get_borrow_udp_socket()
                    .unwrap()
                    .send_to(buf, peer_addr)
                {
                    // We want to write the entire `DATA` buffer in a single go. If we
                    // write less we'll return a short write error (same as
                    // `io::Write::write_all` does).
                    Ok(n) if n < buf.len() => {
                        self.is_write_able = false;
                        // 把没写完的存到buffer里
                        let (_, wait_send) = buf.split_at(n);
                        self.send_buf.put_slice(wait_send);
                        tracing::trace!("Write size: {:?}", n);
                        //
                        // return Err(io::ErrorKind::WriteZero.into());
                        break;
                    }
                    Ok(n) => {
                        // After we've written something we'll reregister the connection
                        // to only respond to readable events.
                        //
                        // registry.reregister(connection, event.token(), Interest::READABLE)?
                        tracing::trace!("Write done size: {:?}", n);
                        break;
                    }

                    Err(ref err) if would_block(err) => {
                        break;
                    }
                    Err(ref err) if interrupted(err) => {
                        continue;
                    }
                    Err(err) => return Err(err),
                }
            } else {
                break;
            }
        }
        // } else {
        //     self.send_buf.put_slice(buf);
        // }
        Ok(())
    }

    pub(crate) fn udp_client_send_data(&mut self, buf: &[u8]) -> io::Result<()> {
        loop {
            if !buf.is_empty() {
                // We can (maybe) write to the connection.
                let peer_addr = self.endpoint.addr;

                match self
                    .endpoint
                    .socket_wrap
                    .get_udp_socket()
                    .unwrap()
                    .send_to(buf, peer_addr)
                {
                    // We want to write the entire `DATA` buffer in a single go. If we
                    // write less we'll return a short write error (same as
                    // `io::Write::write_all` does).
                    Ok(n) if n < buf.len() => {
                        self.is_write_able = false;
                        // 把没写完的存到buffer里
                        let (_, wait_send) = buf.split_at(n);
                        self.send_buf.put_slice(wait_send);
                        tracing::trace!("Write size: {:?}", n);
                        //
                        // return Err(io::ErrorKind::WriteZero.into());
                        break;
                    }
                    Ok(n) => {
                        // After we've written something we'll reregister the connection
                        // to only respond to readable events.
                        //
                        // registry.reregister(connection, event.token(), Interest::READABLE)?
                        tracing::trace!("Write done size: {:?}", n);
                        break;
                    }

                    Err(ref err) if would_block(err) => {
                        break;
                    }
                    Err(ref err) if interrupted(err) => {
                        continue;
                    }
                    Err(err) => return Err(err),
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn tcp_send_data(&mut self, buf: &[u8]) -> io::Result<()> {
        if self.is_write_able {
            loop {
                if !buf.is_empty() {
                    // We can (maybe) write to the connection.
                    match self
                        .endpoint
                        .socket_wrap
                        .get_tcp_stream()
                        .unwrap()
                        .write(buf)
                    {
                        // We want to write the entire `DATA` buffer in a single go. If we
                        // write less we'll return a short write error (same as
                        // `io::Write::write_all` does).
                        Ok(n) if n < buf.len() => {
                            self.is_write_able = false;
                            // 把没写完的存到buffer里
                            let (_, wait_send) = buf.split_at(n);
                            self.send_buf.put_slice(wait_send);
                            tracing::debug!("Write size: {:?}", n);
                            //
                            // return Err(io::ErrorKind::WriteZero.into());
                            break;
                        }
                        Ok(n) => {
                            // After we've written something we'll reregister the connection
                            // to only respond to readable events.
                            //
                            // registry.reregister(connection, event.token(), Interest::READABLE)?
                            tracing::debug!("Write done size: {:?}", n);
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
        } else {
            self.send_buf.put_slice(buf);
        }
        Ok(())
    }
    // 关闭连接
    pub(crate) fn close_connect(&self, how: Shutdown) -> io::Result<()> {
        match self.ep_type {
            EndpointType::TCPListen => {
                return Ok(());
            }
            EndpointType::TCPStream => {
                self.endpoint
                    .socket_wrap
                    .get_tcp_stream()
                    .unwrap()
                    .shutdown(how)?;
            }
            EndpointType::UDPSocket => todo!(),
        }
        Ok(())
    }

    // 获取本地通道地址
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match self.ep_type {
            EndpointType::TCPListen => {
                if self.endpoint.socket_wrap.get_tcp_listener().is_some() {
                    return self
                        .endpoint
                        .socket_wrap
                        .get_tcp_listener()
                        .unwrap()
                        .local_addr();
                } else {
                    Err(std::io::Error::other("Registering"))
                }
            }
            EndpointType::TCPStream => {
                if self.endpoint.socket_wrap.get_tcp_stream().is_some() {
                    return self
                        .endpoint
                        .socket_wrap
                        .get_tcp_stream()
                        .unwrap()
                        .local_addr();
                } else {
                    Err(std::io::Error::other("Registering"))
                }
            }
            EndpointType::UDPSocket => todo!(),
        }
    }

    // 获取对端通道地址
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        match self.ep_type {
            EndpointType::TCPListen => Err(std::io::Error::other("ListenNotPeer")),
            EndpointType::TCPStream => {
                if self.endpoint.socket_wrap.get_tcp_stream().is_some() {
                    return self
                        .endpoint
                        .socket_wrap
                        .get_tcp_stream()
                        .unwrap()
                        .peer_addr();
                } else {
                    Err(std::io::Error::other("Registering"))
                }
            }
            EndpointType::UDPSocket => todo!(),
        }
    }
}

fn would_block(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}

fn interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
}
