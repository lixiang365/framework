use framework::{
    init_io_thread,
    net::{
        self,
        channel::{self},
    },
};
use std::{
    thread::sleep,
    time::{self, Duration},
};

use tracing::{span, Level};

use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn init_log() -> WorkerGuard {
    // 日志
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("logger.gateway")
        .filename_suffix("log")
        .max_log_files(60)
        .build("log")
        .expect("file log init failed!");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    let file_log_subscriber = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true);

    let console_subscriber = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout)
        .with_file(true)
        .with_line_number(true);
    tracing_subscriber::registry()
        .with(file_log_subscriber)
        .with(console_subscriber)
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    _guard
}

fn main() {
    std::env::set_var("RUST_BACKTRACE", "1");
    // 给日志库设置环境变量
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "debug")
    }
	let _guard = init_log();

    init_io_thread(&framework::net::NetConfig::default());
    // sleep(Duration::from_secs(2));
    let addr = "127.0.0.1:9000".parse().unwrap();
    if let Ok(channel_id) = framework::create_server(
        addr,
        framework::net::channel::ChannelOption {
            filter_flags: framework::net::channel::FilterFlags::PACK_MSGID_AND_ENCRYPT,
        },
    ) {
        tracing::info!("create server success channel_id:{}", channel_id);
    } else {
        tracing::error!("create server failed err");
        return;
    }

    let recv = framework::get_recv_msg_channel();

    println!("Hello, world!");
    loop {
        let len = recv.len();
        // tracing::debug!("recv msg len: {}", recv.len());
        for i in 0..len {
            let msg = recv.recv().unwrap();
            tracing::debug!(
                "recv msg channel_id: {} | id:{} | msg:{:?}",
                msg.channel_id,
                msg.msg.msg_id,
                String::from_utf8(msg.msg.buf)
            );
            if msg.msg.msg_id == net::message::FixedMessageId::Connected.as_i32() {
                framework::set_channel_secret_key(
                    msg.channel_id,
                    "12345678901234567890123456789012".to_string(),
                    "1234567890123456".to_string(),
                );
            }

            if msg.msg.msg_id == 1 || msg.msg.msg_id == 0 {
                framework::send_msg(
                    msg.channel_id,
                    net::message::MessagePacket {
                        msg_id: 2,
                        buf: b"server recv!!!".to_vec(),
                    },
                );
            }
        }
        sleep(Duration::from_millis(200));
    }
}
