use framework::{
    init_io_thread,
    net::{self, channel},
};
use std::{
    thread::sleep,
    time::{self, Duration},
};

use tracing::{span, Level};

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn init_log() {
    // 给日志库设置环境变量
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "debug")
    }
    // 日志
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("logger")
        .filename_suffix("log")
        .max_log_files(60)
        .build("log")
        .expect("file log init failed!");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    let file_log_subscriber = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false);

    let console_subscriber = tracing_subscriber::fmt::layer().with_writer(std::io::stdout);
    tracing_subscriber::registry()
        .with(file_log_subscriber)
        .with(console_subscriber)
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();
}

fn main() {
    init_log();
    init_io_thread();
    let addr = "127.0.0.1:9000".parse().unwrap();
    if let Ok(channel_id) = framework::create_server(addr) {
        tracing::info!("create server success channel_id:{}", channel_id);
    }
    let recv = framework::get_recv_msg_channel();

    // if let Ok(channel_id) = net_framework::create_client(addr) {
    //     println!("create client success channel_id:{}", channel_id);
    // }
    println!("Hello, world!");
    loop {
        let len = recv.len();
        tracing::debug!("recv msg len: {}", recv.len());
        for i in 0..len {
            let msg = recv.recv().unwrap();
            // println!("recv msg idx: {} | msg:{:?}", i, msg);
            if msg.msg.msg_id == 1 {
                framework::send_msg(
                    msg.channel_id,
                    net::message::MessagePacket {
                        msg_id: 2,
                        buf: b"server recv!!!".to_vec(),
                    },
                );
            }
        }
        sleep(Duration::from_secs(2));
    }
}
