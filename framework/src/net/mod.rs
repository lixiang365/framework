pub mod channel;
pub(crate) mod driver;
pub mod endpoint;
pub mod message;
pub(crate) mod reactor;

// 引入
use crossbeam::channel as cb_channel;
use driver::Driver;
use mio::Registry;
use reactor::Reactor;
use std::cell::RefCell;

// 全局变量
pub(crate) static mut SEND_MSG_CH_S: Option<cb_channel::Sender<message::MessageItem>> = None;
pub(crate) static mut SEND_MSG_CH_R: Option<cb_channel::Receiver<message::MessageItem>> = None;

pub(crate) static mut RECV_MSG_CH_R: Option<cb_channel::Receiver<message::MessageItem>> = None;
pub(crate) static mut RECV_MSG_CH_S: Option<cb_channel::Sender<message::MessageItem>> = None;

pub(crate) static mut THREAD_TASK_CH_R: Option<cb_channel::Receiver<Box<dyn FnOnce() -> ()>>> =
    None;
pub(crate) static mut THREAD_TASK_CH_S: Option<cb_channel::Sender<Box<dyn FnOnce() -> ()>>> = None;

pub(crate) static mut REGISTRY: Option<Registry> = None;

// 通道缓冲区大小
pub(crate) static mut CH_SEND_BUF_SIZE: usize = 10 * 1024;
pub(crate) static mut CH_RECV_BUF_SIZE: usize = 10 * 1024;

thread_local! {
    pub static DRIVER: RefCell<Driver> = RefCell::new(Driver::new().expect("driver init err - os err"));
    pub static REACTOR: RefCell<Reactor> = RefCell::new(Reactor::new());
}

// ----------------------
// 网络配置
pub struct NetConfig {
    pub ch_send_buf_size: usize,
    pub ch_recv_buf_size: usize,
    pub poll_timeout_ms: usize,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            ch_send_buf_size: 10 * 1024,
            ch_recv_buf_size: 10 * 1024,
            poll_timeout_ms: 5,
        }
    }
}
