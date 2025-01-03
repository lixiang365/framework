

use net::{
    channel::{CH_STATUS_CONNECTED, CH_STATUS_DEFAULT},
    endpoint::EndpointType,
    message::MessagePacket,
};
use reactor::Reactor;
use std::{io, thread};
use tokio_util::codec::Encoder;

pub use crossbeam::channel as cb_channel;
pub use net::message;

//----------------------------------
pub(crate) mod driver;
pub(crate) mod util;
pub(crate) mod reactor;
pub mod net;
pub mod timer;

// 忽略读写锁中毒
macro_rules! rwlock {
    ($lock_result:expr) => {{
        $lock_result.unwrap_or_else(|e| e.into_inner())
    }};
}

// 初始化IO线程
pub fn init_io_thread() {
    thread::Builder::new()
        .name("io_thread".to_string())
        .spawn(move || {
            let mut driver = rwlock!(Reactor::get().driver.write());
            driver.run()
        })
        .expect("cannot spawn io thread");
}

pub fn create_server(addr: std::net::SocketAddr) -> io::Result<usize> {
    // let addr: std::net::SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let token = Reactor::get().next_token();
    let listen = net::endpoint::TcpListener::bind(addr).expect("bind failed ");
    let ep = net::endpoint::Endpoint::new_tcp_listen(listen, addr, token)?;
    let channel = net::channel::Channel::new(ep, token, 0);
    Reactor::get().register_channel(channel)?;
    Ok(token)
}

pub fn create_client(addr: std::net::SocketAddr) -> io::Result<usize> {
    // let addr: std::net::SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let token = Reactor::get().next_token();
    let client = net::endpoint::TcpStream::connect(addr)?;
    let ep = net::endpoint::Endpoint::new_tcp_client(client, addr, token)?;
    let channel = net::channel::Channel::new(ep, token, 0);
    Reactor::get().register_channel(channel)?;
    Ok(token)
}

pub fn get_recv_msg_channel() -> cb_channel::Receiver<message::MessageItem> {
    Reactor::get().recv_msg_channel_r.clone()
}

// 发送数据
// 0 成功（可能立即写入系统缓冲区，也可能在程序缓冲区）
// -1 通道不存在
// -2 通道正在
// -3 通道已经关闭
// -4 发送出错
pub fn send_data(channel_id: usize, buf: &[u8]) -> i32 {
    if let Some(channel) = Reactor::get().channel_map.get(&channel_id) {
        let status = channel.status.load(std::sync::atomic::Ordering::Relaxed);
        if status == CH_STATUS_CONNECTED {
            match channel.send_data(buf) {
                Ok(_) => {
                    // print!("send data success");
                    return 0;
                }
                Err(e) => {
                    print!("send data err:{:?}", e);
                    return -4;
                }
            }
        } else if status == CH_STATUS_DEFAULT {
            return -2;
        } else {
            return -3;
        }
    } else {
        return -1;
    }
}

// 发送消息
// -10 数据太长
pub fn send_msg(channel_id: usize, msg: MessagePacket) -> i32 {
    let mut encoder = message::MessagePacketCodec {};
    let mut buf = bytes::BytesMut::new();
    match encoder.encode(msg, &mut buf) {
        Ok(_) => {
            return send_data(channel_id, &buf[..buf.len()]);
        }
        Err(e) => {
            print!("send_msg - data too long err:{}", e);
			return -10;
        }
    }
}

// 断开客户端连接
pub fn disconnect_client(channel_id: usize, is_wait_send: bool) {
    if let Some(channel) = Reactor::get().channel_map.get(&channel_id) {
        if channel.ep_type == EndpointType::TCPListen {
            return;
        }
    }
    Reactor::get().deregister_channel(channel_id, std::net::Shutdown::Both, is_wait_send);
}



// 创建网络框架

// 创建服务器

// 创建客户端

// 客户端连接
