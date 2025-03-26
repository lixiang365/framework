//! Reactor
//!
//!
use dashmap::DashSet;
use rustc_hash::FxHashMap;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::net::channel::{Channel, CH_STATUS_CONDEMN_AND_WAIT_DESTROY};
use crate::net::message;
use crate::net::message::MessageItem;
use crate::util::static_mut_ref;
use crate::RECV_MSG_CH_S;

static mut TOKEN: AtomicUsize = AtomicUsize::new(10);
// 已经使用的通道id
static CHANNEL_IDS: OnceLock<DashSet<usize>> = OnceLock::new();

pub struct Reactor {
    // 通道管理
    // DashMap 一定要注意,无论可变还是不可变引用保持最小作用域
    pub channel_map: FxHashMap<usize, Channel>,
    // 等待关闭的通道
    wait_close_ch: DashSet<usize>,
}

impl Reactor {
    pub fn new() -> Self {
        Reactor {
            wait_close_ch: DashSet::new(),
            channel_map: FxHashMap::default(),
        }
    }

    pub fn register_channel(&mut self, mut channel: Channel) -> io::Result<()> {
        channel.init()?;
        let msg = message::MessageItem {
            server_id: channel.server_id,
            channel_id: channel.channel_id,
            msg: message::MessagePacket {
                msg_id: message::FixedMessageId::Connected.as_i32(),
                buf: vec![],
            },
        };
        let channel_id = channel.channel_id;
        self.channel_map.insert(channel_id, channel);
        get_channel_ids().insert(channel_id);
        // 通知上层有个新连接
        let _ = static_mut_ref!(RECV_MSG_CH_S).as_ref().unwrap().send(msg);
        Ok(())
    }

    pub(crate) fn deregister_channel(
        &mut self,
        channel_id: usize,
        how: std::net::Shutdown,
        is_wait_send: bool,
    ) {
        let channel = self.channel_map.get_mut(&channel_id);
        let mut is_remove = false;
        if let Some(ch) = channel {
            if !is_wait_send || ch.send_buf.is_empty() {
                // 断开连接
                let ret = ch.close_connect(how);
                tracing::debug!("deregister_channel - ret: {:?}", ret);
                is_remove = true;
                // 删除通道
            } else {
                // 下一帧再删除,数据发送完
                {
                    ch.status = CH_STATUS_CONDEMN_AND_WAIT_DESTROY;
                    self.add_waiting_to_close_ch(channel_id);
                }
            }
        }
        if is_remove {
            self.channel_map.remove(&channel_id);
            self.wait_close_ch.remove(&channel_id);
            get_channel_ids().remove(&channel_id);
            tracing::debug!(
                "deregister_channel - remove channel_id:{} | remain channel num: {:?}",
                channel_id,
                self.channel_map.len()
            );
        }
    }

    // 对方关闭
    pub fn ch_peer_closed(&mut self, server_id: usize, channel_id: usize) {
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
        let _ = static_mut_ref!(RECV_MSG_CH_S).as_ref().unwrap().send(msg);
        // 解除通道注册
        self.deregister_channel(channel_id, std::net::Shutdown::Both, false);
    }

    // 添加待关闭通道
    pub fn add_waiting_to_close_ch(&self, channel_id: usize) {
        self.wait_close_ch.insert(channel_id);
    }

    // 处理待关闭通道
    pub fn process_wait_close_ch(&mut self) {
        let mut ids = Vec::new();
        {
            for id in self.wait_close_ch.iter() {
                let channel = self.channel_map.get(&id);
                if let Some(ch) = channel {
                    if ch.send_buf.is_empty() {
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
    // pub fn client_connect_server_failed(&mut self, server_id: usize, channel_id: usize) {
    //     // 通知上层
    //     let msg = MessageItem {
    //         server_id: server_id,
    //         channel_id: channel_id,
    //         msg: message::MessagePacket {
    //             msg_id: message::FixedMessageId::ConnectFiled.as_i32(),
    //             buf: vec![],
    //         },
    //     };
    //     // 通知主线程有个客户端连接失败
    //     let _ = unsafe { RECV_MSG_CH_S.as_ref().unwrap().send(msg) };
    //     // 解除通道注册
    //     self.deregister_channel(channel_id, std::net::Shutdown::Both, false);
    // }
}

fn get_channel_ids() -> &'static DashSet<usize> {
    CHANNEL_IDS.get_or_init(|| {
        // 假设这块初始化操作是昂贵的
        DashSet::new()
    });
    // 返回初始化后的值
    CHANNEL_IDS.get().unwrap()
}

pub fn next_token() -> usize {
    loop {
        let token = static_mut_ref!(TOKEN).fetch_add(1, Ordering::SeqCst);
        {
            if get_channel_ids().contains(&token) && token < 10 {
                continue;
            }
            return token;
        }
    }
}
