use chrono::prelude::*;
use framework::{init_io_thread, timer};

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
    // if let Ok(channel_id) = net_framework::create_server(addr) {
    //     println!("create server success channel_id:{}", channel_id);
    // }
    let recv = framework::get_recv_msg_channel();
    let timerMgr = timer::TimerManager::new();
    use std::time::{Duration, Instant};
    println!("Hello, world! now: {:?}", Utc::now().timestamp_millis());
    timerMgr.insert_timer(
        1,
        Instant::now() + Duration::from_secs(5),
        Box::new(|| {
            println!("Hello, world! after 5 now: {:?}", Utc::now().timestamp_millis());
        }),
    );


    for i in 0..1 {
        if let Ok(id) = framework::create_client(addr) {
            println!("create client success channel_id:{}", id);
        } else {
            println!("create client failed channel_id:");
        }
    }
    loop {
        let len = recv.len();
        // println!("recv msg len: {}", len);
        // for i in 0..len {
        //     let msg = recv.recv().unwrap();
        //     if msg.msg.msg_id == net_framework::message::FixedMessageId::Connected.as_i32() {
        //         net_framework::send_msg(
        //             msg.channel_id,
        //             net_framework::message::MessagePacket {
        //                 msg_id: 1,
        //                 buf: [0; 100].to_vec(),
        //             },
        //         );
        //     }
        //     // println!("recv msg idx: {} | msg:{:?}", i, recv.recv().unwrap());
        // }
        let mut cb = Vec::new();
        timerMgr.process_timers(&mut cb);
        if cb.len() > 0 {
            cb[0]();
        }
        // sleep(Duration::from_secs(1));
    }
}
