use std::thread::sleep;

use chrono::prelude::*;
use framework::{init_io_thread, timer};

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn init_log() {
    std::env::set_var("RUST_BACKTRACE", "1");

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
    init_io_thread(&framework::net::NetConfig::default());
    let addr = "127.0.0.1:9000".parse().unwrap();

    let recv = framework::get_recv_msg_channel();
    let timer_mgr = timer::TimerManager::new();
    use std::time::{Duration, Instant};
    println!("Hello, world! now: {:?}", Utc::now().timestamp_millis());
    timer_mgr.insert_timer(
        Duration::from_secs(5),
        Box::new(|| {
            println!(
                "Hello, world! after 5 now: {:?}",
                Utc::now().timestamp_millis()
            );
        }),
        false,
    );

    for _ in 0..1 {
        let _ = std::thread::Builder::new()
            .name("io_thread".to_string())
            .spawn(move || {
                for _ in 0..1 {
                    match framework::create_udp_client(
                        addr,
                        framework::net::channel::ChannelOption {
                            filter_flags: framework::net::channel::FilterFlags::RAW_BYTE,
                        },
                    ) {
                        Ok(id) => {
                            println!("create client success channel_id:{}", id);
                        }
                        Err(e) => {
                            println!("create client failed e:{:?}", e);
                        }
                    }
                }
            });
    }
    loop {
        let len = recv.len();
        // println!("recv msg len: {}", len);
        for i in 0..len {
            let msg = recv.recv().unwrap();
            println!(
                "recv msg idx: {} | msg:{:?}",
                i,
                String::from_utf8(msg.msg.buf)
            );
            if msg.msg.msg_id == framework::net::message::FixedMessageId::Connected.as_i32() || true
            {
                framework::send_msg(
                    msg.channel_id,
                    framework::net::message::MessagePacket {
                        msg_id: 1,
                        buf: format!("client id: {} - ping", msg.channel_id)
                            .as_bytes()
                            .to_vec(),
                    },
                );
            }
            // println!("recv msg idx: {} | msg:{:?}", i, recv.recv().unwrap());
        }
        timer_mgr.update();
        sleep(Duration::from_millis(1000));
    }
}
