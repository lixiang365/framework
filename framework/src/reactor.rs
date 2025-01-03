//! Reactor
//!
//!
use async_lock::OnceCell;
use dashmap::{DashMap, DashSet};
use mio::Registry;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;

use crate::net::channel::{Channel, CH_STATUS_CONDEMN_AND_WAIT_DESTROY};
use crate::net::message::MessageItem;
use crate::{driver, message};
use crossbeam::channel as cb_channel;

use self::driver::Driver;

macro_rules! rwlock {
    ($lock_result:expr) => {{
        $lock_result.unwrap_or_else(|e| e.into_inner())
    }};
}

pub struct Reactor {
    pub driver: RwLock<Driver>,
    pub registry: Registry,
    // 通道管理
    // DashMap 一定要注意,无论可变还是不可变引用保持最小作用域
    pub channel_map: DashMap<usize, Channel>,
    // 接收消息 发送到外部通道
    pub recv_msg_channel_s: cb_channel::Sender<MessageItem>,
    pub recv_msg_channel_r: cb_channel::Receiver<MessageItem>,
    token: AtomicUsize,
    // 等待关闭的通道
    wait_close_ch: DashSet<usize>,
}

impl Reactor {
    pub fn get() -> &'static Self {
        static REACTOR: OnceCell<Reactor> = OnceCell::new();
        REACTOR.get_or_init_blocking(|| {
            let driver = driver::Driver::new().expect("driver init err - os err");
            let registry = driver
                .poller
                .registry()
                .try_clone()
                .expect("driver poller registry clone err");
            let (s1, r1) = cb_channel::unbounded();
            Reactor {
                driver: RwLock::new(driver),
                registry,
                token: AtomicUsize::new(10),
                recv_msg_channel_s: s1,
                recv_msg_channel_r: r1,
                wait_close_ch: DashSet::new(),
                channel_map: DashMap::new(),
            }
        })
    }

    pub fn register_channel(&self, channel: Channel) -> io::Result<()> {
        let channel_id = channel.channel_id;
        self.channel_map.insert(channel_id, channel);
        let mut err = None;
        if let Some(mut channel) = self.channel_map.get_mut(&channel_id) {
            match channel.init() {
                Ok(_) => {
                    return Ok(());
                }
                Err(e) => {
                    err = Some(e);
                }
            }
        }
        if err.is_some() {
            self.channel_map.remove(&channel_id);
            return Err(err.take().unwrap());
        } else {
            Ok(())
        }
    }

    pub(crate) fn deregister_channel(
        &self,
        channel_id: usize,
        how: std::net::Shutdown,
        is_wait_send: bool,
    ) {
        let channel = self.channel_map.get(&channel_id);
        let mut is_remove = false;
        if let Some(ch) = channel {
            let send_buf = rwlock!(ch.send_buf.read());
            if !is_wait_send || send_buf.is_empty() {
                // 断开连接
                let ret = ch.close_connect(how);
                tracing::debug!("deregister_channel - ret: {:?}", ret);
                is_remove = true;
                // 删除通道
            } else {
                // 下一帧再删除,数据发送完
                {
                    ch.status
                        .store(CH_STATUS_CONDEMN_AND_WAIT_DESTROY, Ordering::Relaxed);
                    self.add_waiting_to_close_ch(channel_id);
                }
            }
        }
        if is_remove {
            self.channel_map.remove(&channel_id);
            self.wait_close_ch.remove(&channel_id);
            tracing::debug!(
                "deregister_channel - remain channel num: {:?}",
                self.channel_map.len()
            );
        }
    }

    // 对方关闭
    pub fn ch_peer_closed(&self, server_id: usize, channel_id: usize) {
        // 通知上层
        let msg = MessageItem {
            server_id: server_id,
            channel_id: channel_id,
            msg: message::MessagePacket {
                msg_id: message::FixedMessageId::DisConnect.as_i32(),
                buf: vec![],
            },
        };
        // 通知主线程有个连接
        let _ = self.recv_msg_channel_s.send(msg);
        // 解除通道注册
        self.deregister_channel(channel_id, std::net::Shutdown::Both, false);
    }

    // 添加待关闭通道
    pub fn add_waiting_to_close_ch(&self, channel_id: usize) {
        self.wait_close_ch.insert(channel_id);
    }

    // 处理待关闭通道
    pub fn process_wait_close_ch(&self) {
        let mut ids = Vec::new();
        {
            for id in self.wait_close_ch.iter() {
                let channel = self.channel_map.get(&id);
                if let Some(ch) = channel {
                    let buf = rwlock!(ch.send_buf.read());
                    if buf.is_empty() {
                        ids.push(id.clone());
                    }
                }
            }
        }
        for id in ids {
            self.deregister_channel(id, std::net::Shutdown::Both, false);
        }
    }

    // 作为客户端连接服务器失败
    pub fn client_connect_server_failed(&self, server_id: usize, channel_id: usize) {
        // 通知上层
        let msg = MessageItem {
            server_id: server_id,
            channel_id: channel_id,
            msg: message::MessagePacket {
                msg_id: message::FixedMessageId::ConnectFiled.as_i32(),
                buf: vec![],
            },
        };
        // 通知主线程有个客户端连接失败
        let _ = self.recv_msg_channel_s.send(msg);
        // 解除通道注册
        self.deregister_channel(channel_id, std::net::Shutdown::Both, false);
    }

    pub fn next_token(&self) -> usize {
        loop {
            let token = self.token.fetch_add(1, Ordering::SeqCst);
            {
                if !self.channel_map.contains_key(&token) && token != 0 {
                    return token;
                }
            }
        }
    }
}
