//! 驱动器
//!
//!	执行事件
use super::channel::{ChannelOption, CH_STATUS_CONNECTED};
use super::endpoint::Endpoint;
use super::{reactor, SEND_MSG_CH_R};
use crate::net::REACTOR;
use crate::net::{channel::Channel, endpoint::EndpointType};
use crate::util::static_mut_ref;
use bytes::BufMut;
use mio::{event::Event, Events, Poll, Token};
use std::{io, time::Duration};
use tracing::{span, Level};

pub struct Driver {
    pub poller: Poll,
    pub events: Events,
    poll_timeout: Duration,
}

impl Driver {
    pub fn new() -> io::Result<Driver> {
        let poll = Poll::new().expect("Driver init poll err");
        let events = Events::with_capacity(1024);
        Ok(Self {
            poller: poll,
            events: events,
            poll_timeout: Duration::from_millis(5),
        })
    }

    pub fn set_poll_timeout(&mut self, poll_timeout_ms: usize) {
        self.poll_timeout = Duration::from_millis(poll_timeout_ms.try_into().unwrap());
    }

    pub(crate) fn run_once(&mut self) {
        let _ = self.run_once_handle_event();
        let _ = self.run_once_send_msg();
    }

    pub fn run_once_handle_event(&mut self) -> io::Result<()> {
        // Start an event loop.
        if let Err(err) = self
            .poller
            .poll(&mut self.events, Some(Duration::from_millis(20)))
        {
            if interrupted(&err) {
                tracing::error!("poll interrupted ....");
                return Ok(());
            }
            tracing::error!("poll err:{:?}", err);
            return Err(err);
        }
        // tracing::debug!("polling event len: {:?}",self.events.is_empty());
        for event in self.events.iter() {
            let _ = handle_event(event.token(), event);
        }
        // 检查
        REACTOR.with_borrow_mut(|reactor| reactor.process_wait_close_ch());
        Ok(())
    }

    // 发送消息
    pub(crate) fn run_once_send_msg(&mut self) {
        // 先处理发送消息通道，
        let len = static_mut_ref!(SEND_MSG_CH_R).as_ref().unwrap().len();
        for _ in 0..len {
            let msg = static_mut_ref!(SEND_MSG_CH_R).as_ref().unwrap().recv();
            if let Ok(msg) = msg {
                REACTOR.with_borrow_mut(|reactor| {
                    if let Some(channel) = reactor.channel_map.get_mut(&msg.channel_id) {
                        match channel.send_msg_filter(msg.msg) {
                            Ok(_) => {}
                            Err(e) => {
                                tracing::error!("send data err:{:?}", e);
                            }
                        }
                    } else {
                        tracing::error!("send data channel :{} is not exist", msg.channel_id);
                    }
                });
            }
        }
    }
}

// 事件处理
fn handle_event(token: Token, event: &Event) -> io::Result<()> {
    let span = span!(Level::TRACE, "net event handle");
    let _guard = span.enter();
    // 从reactor 拿到tcp
    let mut ep_type = EndpointType::TCPStream;
    let mut server_id = 0;
    let mut channel_id = 0;
    let mut channel_status = 0;
    REACTOR.with_borrow(|reactor| {
        if let Some(channel) = reactor.channel_map.get(&token.0) {
            ep_type = channel.ep_type;
            server_id = channel.server_id;
            channel_id = channel.channel_id;
            channel_status = channel.status;
        } else {
            tracing::error!("handle_event - channel is not exist id:{:?}", token);
        }
    });
    if channel_id == 0 {
        return Ok(());
    }

    if event.is_error() || event.is_read_closed() {
        // 关闭通道
        tracing::debug!(
            "handle_event - err:{} | read_close{}",
            event.is_error(),
            event.is_read_closed()
        );
        if ep_type == EndpointType::TCPStream {
            REACTOR.with_borrow_mut(|reactor| reactor.ch_peer_closed(server_id, channel_id));
            return Ok(());
        }
    }
    match ep_type {
        crate::net::endpoint::EndpointType::TCPListen => {
            let mut new_channels: Option<Vec<Channel>> = None;
            REACTOR.with_borrow_mut(|reactor| {
                let channel = reactor.channel_map.get(&token.0).unwrap();
                match channel.accept_new_conn() {
                    Ok(chs) => {
                        new_channels = Some(chs);
                    }
                    Err(e) => {
                        // accept报错
                        tracing::error!("accept err:{:?}", e);
                    }
                }
                if let Some(channels) = new_channels {
                    for new_ch in channels {
                        match reactor.register_channel(new_ch) {
                            Ok(_) => {}
                            Err(e) => {
                                tracing::error!("handle_event register new ch err:{:?}", e);
                            }
                        }
                    }
                }
            });
            ()
        }
        crate::net::endpoint::EndpointType::TCPStream => {
            let mut disconnect = false;
            // let mut connect_failed = false;
            REACTOR.with_borrow_mut(|reactor| {
                if let Some(channel) = reactor.channel_map.get_mut(&token.0) {
                    match channel.handle_connection_event(event) {
                        Ok(_) => {}
                        Err(e) => {
                            match e.kind() {
                                std::io::ErrorKind::Other => {
                                    if e.to_string() == "ConClosed" {
                                        // 对方断开连接
                                        disconnect = true;
                                    }
                                }
                                _ => {}
                            }
                            tracing::error!("handle_connection_event - err:{:?}", e)
                        }
                    }
                }
                if disconnect {
                    reactor.ch_peer_closed(server_id, channel_id);
                }
            });
        }
        crate::net::endpoint::EndpointType::UDPSocket => {
            let mut disconnect = false;
            REACTOR.with_borrow_mut(|reactor| {
                if reactor.channel_map.contains_key(&channel_id) {
                    let ch = reactor.channel_map.get_mut(&token.0).unwrap();
                    let mut peer_addr_data: Option<(std::net::SocketAddr, Vec<u8>)> = None;
                    let mut is_listen = false;
                    if ch.is_server_listen() {
                        // 直接读取数据
                        if let Ok((peer, data)) = ch.udp_listen_handle_event(event) {
                            peer_addr_data = Some((peer, data));
                        }
                        is_listen = true;
                    }
                    if is_listen {
                        if let Some((peer, data)) = peer_addr_data {
                            let mut is_find = false;
                            for (_, client_ch) in &mut reactor.channel_map {
                                if client_ch.endpoint.addr == peer {
                                    client_ch.recv_buf.put_slice(&data);
                                    let _ = client_ch.recv_msg_filter();
                                    is_find = true;
                                    break;
                                }
                            }
                            // 创建新的通道
                            if !is_find {
                                let ch = reactor.channel_map.get_mut(&token.0).unwrap();
                                let channel_id = ch.channel_id;
                                let filter_flags = ch.filter_flags;
                                let source = ch
                                    .endpoint
                                    .socket_wrap
                                    .get_std_udp_socket()
                                    .unwrap()
                                    .try_clone()
                                    .unwrap();
                                let token = reactor::next_token();
                                let socket = crate::net::endpoint::UdpSocket::from_std(source);
                                let new_ep =
                                    Endpoint::new_borrow_udp_socket(socket, peer, token).unwrap();
                                let mut new_ch = Channel::new(
                                    &ChannelOption {
                                        filter_flags: filter_flags,
                                    },
                                    new_ep,
                                    token,
                                    channel_id,
                                );
                                new_ch.status = CH_STATUS_CONNECTED;
                                let _ = reactor.register_channel(new_ch);
                                let ch = reactor.channel_map.get_mut(&token).unwrap();
                                ch.recv_buf.put_slice(&data);
                                let _ = ch.recv_msg_filter();
                            }
                        }
                    } else {
                        match ch.handle_connection_event(event) {
                            Ok(_) => {}
                            Err(e) => {
                                match e.kind() {
                                    std::io::ErrorKind::Other => {
                                        if e.to_string() == "connect failed" {
                                        } else {
                                            // 对方断开连接
                                            disconnect = true;
                                        }
                                    }
                                    _ => {}
                                }
                                tracing::error!("handle_connection_event - err:{:?}", e)
                            }
                        }
                    }
                }
                if disconnect {
                    reactor.ch_peer_closed(server_id, channel_id);
                }
            });
        }
    }
    Ok(())
}

fn interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
}
