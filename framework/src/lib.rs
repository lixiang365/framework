pub mod net;
pub mod timer;
pub mod util;

//----------------------------------
use net::message::{MessageItem, MessagePacket};
use net::reactor;
use net::{
    NetConfig, CH_RECV_BUF_SIZE, CH_SEND_BUF_SIZE, DRIVER, REACTOR, RECV_MSG_CH_R, RECV_MSG_CH_S,
    REGISTRY, SEND_MSG_CH_R, SEND_MSG_CH_S, THREAD_TASK_CH_R, THREAD_TASK_CH_S,
};
use std::{io, thread};
use util::static_mut_ref;

pub use crossbeam::channel as cb_channel;

// 初始化IO线程
pub fn init_io_thread(config: &NetConfig) {
    let (s1, r1) = cb_channel::unbounded();
    unsafe {
        RECV_MSG_CH_R = Some(r1);
        RECV_MSG_CH_S = Some(s1);
    }
    let (s2, r2) = cb_channel::unbounded();
    unsafe {
        THREAD_TASK_CH_R = Some(r2);
        THREAD_TASK_CH_S = Some(s2);
    }
    let (s3, r3) = cb_channel::unbounded();
    unsafe {
        SEND_MSG_CH_S = Some(s3);
        SEND_MSG_CH_R = Some(r3);
    };

    unsafe {
        CH_SEND_BUF_SIZE = config.ch_send_buf_size;
        CH_RECV_BUF_SIZE = config.ch_recv_buf_size;
    }
    let poll_timeout_ms = config.poll_timeout_ms;
    thread::Builder::new()
        .name("io_thread".to_string())
        .spawn(move || {
            DRIVER.with_borrow_mut(|d| {
                let registry = d
                    .poller
                    .registry()
                    .try_clone()
                    .expect("init_io_thread - driver poller registry clone err");
                unsafe {
                    REGISTRY = Some(registry);
                }
                d.set_poll_timeout(poll_timeout_ms);
            });
            let thread_task_ch_r = static_mut_ref!(THREAD_TASK_CH_R).as_ref().unwrap().clone();

            loop {
                // poller事件
                let _ = DRIVER.with_borrow_mut(|driver| driver.run_once());
                // 任务通道
                let task_len = thread_task_ch_r.len();
                for _ in 0..task_len {
                    if let Ok(task) = thread_task_ch_r.recv() {
                        task();
                    }
                }
            }
        })
        .expect("cannot spawn io thread");
}

pub fn create_server(
    addr: std::net::SocketAddr,
    ch_option: net::channel::ChannelOption,
) -> io::Result<usize> {
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    let token = reactor::next_token();
    let task = move || {
        REACTOR.with_borrow_mut(|reactor| {
            // let listen = net::endpoint::TcpListener::bind(addr).expect("bind failed ");
            let listen = net::endpoint::TcpListener::from_std(listener);
            let ep = net::endpoint::Endpoint::new_tcp_listen(listen, addr, token).unwrap();
            let channel = net::channel::Channel::new(&ch_option, ep, token, token);
            let ret = reactor.register_channel(channel);
            if ret.is_ok() {
                tracing::info!(
                    "listen server success - addr:{} | channel_id:{} ",
                    addr.to_string(),
                    token
                );
            } else {
                tracing::error!(
                    "listen server failed - addr:{} | channel_id:{} | err:{:?}",
                    addr.to_string(),
                    token,
                    ret
                );
            }
        })
    };
    let _ = static_mut_ref!(THREAD_TASK_CH_S)
        .as_ref()
        .unwrap()
        .send(Box::new(task));
    Ok(token)
}

pub fn create_client(
    addr: std::net::SocketAddr,
    ch_option: net::channel::ChannelOption,
) -> io::Result<usize> {
    let tcp_stream = std::net::TcpStream::connect(addr)?;
    tcp_stream.set_nonblocking(true)?;
    let token = reactor::next_token();
    let task = move || {
        REACTOR.with_borrow_mut(|reactor| {
            // let client = net::endpoint::TcpStream::connect(addr).unwrap();
            let client = net::endpoint::TcpStream::from_std(tcp_stream);
            let ep = net::endpoint::Endpoint::new_tcp_client(client, addr, token).unwrap();
            let channel = net::channel::Channel::new(&ch_option, ep, token, 0);
            let ret = reactor.register_channel(channel);
            if ret.is_ok() {
                tracing::info!(
                    "create client success - addr:{} | channel_id:{} ",
                    addr.to_string(),
                    token
                );
            } else {
                tracing::error!(
                    "create client failed - addr:{} | channel_id:{} | err:{:?}",
                    addr.to_string(),
                    token,
                    ret
                );
            }
        })
    };

    let _ = static_mut_ref!(THREAD_TASK_CH_S)
        .as_ref()
        .unwrap()
        .send(Box::new(task));
    Ok(token)
}

pub fn create_udp_server(
    addr: std::net::SocketAddr,
    ch_option: net::channel::ChannelOption,
) -> io::Result<usize> {
    let listener = std::net::UdpSocket::bind(addr)?;
    listener.set_nonblocking(true)?;
    let token = reactor::next_token();
    let socket = listener.try_clone().unwrap();
    let task = move || {
        REACTOR.with_borrow_mut(|reactor| {
            // let listen = net::endpoint::TcpListener::bind(addr).expect("bind failed ");
            let listen = net::endpoint::UdpSocket::from_std(listener);
            let ep = net::endpoint::Endpoint::new_udp_socket(socket, listen, addr, token).unwrap();
            let channel = net::channel::Channel::new(&ch_option, ep, token, token);
            let ret = reactor.register_channel(channel);
            if ret.is_ok() {
                tracing::info!(
                    "listen server success - addr:{} | channel_id:{} ",
                    addr.to_string(),
                    token
                );
            } else {
                tracing::error!(
                    "listen server failed - addr:{} | channel_id:{} | err:{:?}",
                    addr.to_string(),
                    token,
                    ret
                );
            }
        })
    };
    let _ = static_mut_ref!(THREAD_TASK_CH_S)
        .as_ref()
        .unwrap()
        .send(Box::new(task));
    Ok(token)
}

pub fn create_udp_client(
    addr: std::net::SocketAddr,
    ch_option: net::channel::ChannelOption,
) -> io::Result<usize> {
    let udp_stream = std::net::UdpSocket::bind("127.0.0.1:0")?;
    udp_stream.connect(addr)?;
    udp_stream.set_nonblocking(true)?;
    let token = reactor::next_token();
    let socket = udp_stream.try_clone().unwrap();
    let task = move || {
        REACTOR.with_borrow_mut(|reactor| {
            // let client = net::endpoint::TcpStream::connect(addr).unwrap();
            let client = net::endpoint::UdpSocket::from_std(udp_stream);
            let ep = net::endpoint::Endpoint::new_udp_socket(socket, client, addr, token).unwrap();
            let channel = net::channel::Channel::new(&ch_option, ep, token, 0);
            let ret = reactor.register_channel(channel);
            if ret.is_ok() {
                tracing::info!(
                    "create client success - addr:{} | channel_id:{} ",
                    addr.to_string(),
                    token
                );
            } else {
                tracing::error!(
                    "create client failed - addr:{} | channel_id:{} | err:{:?}",
                    addr.to_string(),
                    token,
                    ret
                );
            }
        })
    };

    let _ = static_mut_ref!(THREAD_TASK_CH_S)
        .as_ref()
        .unwrap()
        .send(Box::new(task));
    Ok(token)
}

pub fn create_udp_broadcast(
    addr: std::net::SocketAddr,
    ch_option: net::channel::ChannelOption,
) -> io::Result<usize> {
    let udp_stream = std::net::UdpSocket::bind("127.0.0.1:0")?;
    udp_stream.set_broadcast(true)?;
    udp_stream.connect(addr)?;
    udp_stream.set_nonblocking(true)?;
    let token = reactor::next_token();
    let socket = udp_stream.try_clone().unwrap();
    let task = move || {
        REACTOR.with_borrow_mut(|reactor| {
            // let client = net::endpoint::TcpStream::connect(addr).unwrap();
            let client = net::endpoint::UdpSocket::from_std(udp_stream);
            let ep = net::endpoint::Endpoint::new_udp_socket(socket, client, addr, token).unwrap();
            let channel = net::channel::Channel::new(&ch_option, ep, token, 0);
            let ret = reactor.register_channel(channel);
            if ret.is_ok() {
                tracing::info!(
                    "create client success - addr:{} | channel_id:{} ",
                    addr.to_string(),
                    token
                );
            } else {
                tracing::error!(
                    "create client failed - addr:{} | channel_id:{} | err:{:?}",
                    addr.to_string(),
                    token,
                    ret
                );
            }
        })
    };

    let _ = static_mut_ref!(THREAD_TASK_CH_S)
        .as_ref()
        .unwrap()
        .send(Box::new(task));
    Ok(token)
}

pub fn get_recv_msg_channel() -> cb_channel::Receiver<MessageItem> {
    static_mut_ref!(RECV_MSG_CH_R).as_ref().unwrap().clone()
}

// 发送数据
// 0 成功（可能立即写入系统缓冲区，也可能在程序缓冲区）
// -1 通道不存在
// -2 通道正在
// -3 通道已经关闭
// -4 发送出错

pub fn send_msg(channel_id: usize, msg: MessagePacket) -> i32 {
    let msg = MessageItem {
        channel_id: channel_id,
        msg: msg,
        server_id: 0,
    };
    let ret = static_mut_ref!(SEND_MSG_CH_S).as_ref().unwrap().send(msg);
    if let Err(e) = ret {
        tracing::error!("send_msg -  err:{}", e);
        return -1;
    }
    0
}

// 断开客户端连接
pub fn disconnect_client(channel_id: usize, is_wait_send: bool) {
    let task = move || {
        REACTOR.with_borrow_mut(|reactor| {
            let ret =
                reactor.deregister_channel(channel_id, std::net::Shutdown::Both, is_wait_send);
            tracing::debug!(
                "disconnect_client - ret:{:?} | remain channel num:{:?}",
                ret,
                reactor.channel_map.len()
            );
        })
    };
    let _ = static_mut_ref!(THREAD_TASK_CH_S)
        .as_ref()
        .unwrap()
        .send(Box::new(task));
}

// 设置通道密钥
pub fn set_channel_secret_key(channel_id: usize, key: String, iv: String) {
    let task = move || {
        REACTOR.with_borrow_mut(|reactor| {
            if let Some(ch) = reactor.channel_map.get_mut(&channel_id) {
                ch.set_aes_key(key, iv);
                tracing::debug!("set_channel_secret_key - set key");
            }
        })
    };
    let _ = static_mut_ref!(THREAD_TASK_CH_S)
        .as_ref()
        .unwrap()
        .send(Box::new(task));
}

// 创建网络框架

// 创建服务器

// 创建客户端

// 客户端连接
