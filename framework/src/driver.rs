//! 驱动器
//!	
//!	执行事件
use crate::{
    net::{
        channel::{Channel, CH_STATUS_DEFAULT},
        endpoint::EndpointType,
        message,
    },
    reactor::Reactor,
};
use mio::{event::Event, Events, Poll, Token};
use std::{io, sync::atomic::Ordering, time::Duration};
use tracing::{span, Level};

pub struct Driver {
    pub poller: Poll,
    pub events: Events,
}

impl Driver {
    pub fn new() -> io::Result<Driver> {
        let poll = Poll::new().expect("reator init poll err");
        let events = Events::with_capacity(128);
        Ok(Self {
            poller: poll,
            events: events,
        })
    }

    pub fn run(&mut self) -> io::Result<()> {
        // Start an event loop.
        loop {
            if let Err(err) = self
                .poller
                .poll(&mut self.events, Some(Duration::from_millis(20)))
            {
                if interrupted(&err) {
                    tracing::error!("poll interrupted ....");
                    continue;
                }
                tracing::error!("poll err:{:?}", err);
                return Err(err);
            }

            for event in self.events.iter() {
                let _ = handle_event(event.token(), event);
            }
            // 检查
            Reactor::get().process_wait_close_ch();
        }
    }
}

// 事件处理
fn handle_event(token: Token, event: &Event) -> io::Result<()> {
    let span = span!(Level::TRACE, "net event handle");
    let _guard = span.enter();
    // 从reactor 拿到tcp
    let ep_type;
    let server_id;
    let channel_id;
    let channel_status;
    if let Some(channel) = Reactor::get().channel_map.get(&token.0) {
        ep_type = channel.ep_type;
        server_id = channel.server_id;
        channel_id = channel.channel_id;
        channel_status = channel.status.load(Ordering::Relaxed);
    } else {
        tracing::error!(
            "handle_connection_event - channel is not exist id:{:?}",
            token
        );
        return Ok(());
    }

    if event.is_error() || event.is_read_closed() {
        // 关闭通道
        tracing::debug!(
            "handle_event - err:{} | read_close{}",
            event.is_error(),
            event.is_read_closed()
        );
        if ep_type == EndpointType::TCPClient {
            if channel_status == CH_STATUS_DEFAULT {
                // 连接失败
                Reactor::get().client_connect_server_failed(server_id, channel_id);
            } else {
                Reactor::get().ch_peer_closed(server_id, channel_id);
            }
            return Ok(());
        }
    }
    match ep_type {
        crate::net::endpoint::EndpointType::TCPListen => {
            let mut new_channels: Option<Vec<Channel>> = None;
            if let Some(channel) = Reactor::get().channel_map.get(&token.0) {
                match channel.accept_new_conn() {
                    Ok(chs) => {
                        new_channels = Some(chs);
                    }
                    Err(e) => {
                        // accept报错
                        tracing::error!("accept err:{:?}", e);
                    }
                }
            }
            if let Some(channels) = new_channels {
                for new_ch in channels {
                    let msg = message::MessageItem {
                        server_id: new_ch.server_id,
                        channel_id: new_ch.channel_id,
                        msg: message::MessagePacket {
                            msg_id: message::FixedMessageId::Connected.as_i32(),
                            buf: vec![],
                        },
                    };
                    if Reactor::get().register_channel(new_ch).is_ok() {
                        // 通知上层有个新连接
                        let _ = Reactor::get().recv_msg_channel_s.send(msg);
                    }
                }
            }
        }
        _ => {
            let mut disconnect = false;
            let mut connect_failed = false;
            if let Some(channel) = Reactor::get().channel_map.get(&token.0) {
                match channel.handle_connection_event(event) {
                    Ok(_) => {}
                    Err(e) => {
                        match e.kind() {
                            std::io::ErrorKind::Other => {
                                if e.to_string() == "connect failed" {
                                    connect_failed = true;
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
            if disconnect {
                Reactor::get().ch_peer_closed(server_id, channel_id);
            }
            if connect_failed {
                Reactor::get().client_connect_server_failed(server_id, channel_id);
            }
        }
    }
    Ok(())
}

fn interrupted(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Interrupted
}
