//! 消息包定义和编码
//!
//!

use bytes::BufMut;
use tokio_util::codec;

use crate::util::{aes256_cbc_decrypt, aes256_cbc_encrypt};

#[derive(Debug)]
pub struct MessageItem {
    pub server_id: usize,
    pub channel_id: usize,
    pub msg: MessagePacket,
}

// 固定消息id
pub enum FixedMessageId {
    // 初始化
    Init,
    // 已连接
    Connected,
    // 断开连接
    DisConnect,
    // 作为客户端连接服务器失败
    ConnectFiled,

    FixedMsgIdMax,
}

impl FixedMessageId {
    pub fn as_i32(&self) -> i32 {
        match self {
            FixedMessageId::FixedMsgIdMax => 0,
            FixedMessageId::Init => -1,
            FixedMessageId::Connected => -2,
            FixedMessageId::DisConnect => -3,
            FixedMessageId::ConnectFiled => -4,
        }
    }
}

impl TryInto<i32> for FixedMessageId {
    type Error = ();
    fn try_into(self) -> Result<i32, Self::Error> {
        match self {
            FixedMessageId::FixedMsgIdMax => Ok(0),
            FixedMessageId::Init => Ok(-1),
            FixedMessageId::Connected => Ok(-2),
            FixedMessageId::DisConnect => Ok(-3),
            FixedMessageId::ConnectFiled => Ok(-4),
        }
    }
}

impl TryFrom<i32> for FixedMessageId {
    type Error = ();

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(FixedMessageId::FixedMsgIdMax),
            -1 => Ok(FixedMessageId::Init),
            -2 => Ok(FixedMessageId::Connected),
            -3 => Ok(FixedMessageId::DisConnect),
            -4 => Ok(FixedMessageId::ConnectFiled),
            // FixedMessageId::Init => Ok(-1),
            // FixedMessageId::Connected => Ok(-2),
            // FixedMessageId::DisConnect => Ok(-3),
            // FixedMessageId::ConnectFiled => Ok(-4),
            _ => Err(()),
        }
    }
}

// 自定义的消息包协议
#[derive(Debug, Clone)]
pub struct MessagePacket {
    pub msg_id: i32,
    pub buf: Vec<u8>,
}

pub struct MessagePacketCodec {}
impl MessagePacketCodec {
    const MAX_SIZE: usize = 8 * 1024 * 1024 as usize;
}

impl codec::Encoder<MessagePacket> for MessagePacketCodec {
    type Error = std::io::Error;
    fn encode(
        &mut self,
        item: MessagePacket,
        dst: &mut bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        let data = item.buf.as_slice();
        let data_len = data.len() + 4;

        if data_len > Self::MAX_SIZE {
            return Err(std::io::Error::other("frame is too large"));
        }
        dst.reserve(data_len + 4);
        // 1. 写入消息长度
        dst.put_u32(data_len as u32);
        // 2. 写入消息id
        dst.put_u32(item.msg_id as u32);
        // 2. 再将实际数据放入帧尾
        dst.extend_from_slice(data);
        Ok(())
    }
}

impl codec::Decoder for MessagePacketCodec {
    type Item = MessagePacket;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let buf_len = src.len();

        // 如果buf中的数据量连长度声明的大小都不足，则先跳过等待后面更多数据的到来
        if buf_len < 4 {
            return Ok(None);
        }

        // 先读取帧首，获得声明的帧中实际数据大小
        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(&src[..4]);
        let data_len = u32::from_be_bytes(length_bytes) as usize;
        if data_len > Self::MAX_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Frame of length {} is too large.", data_len),
            ));
        }

        // 帧的总长度为 4 + frame_len
        let frame_len = data_len + 4;

        // buf中数据量不够，跳过，并预先申请足够的空闲空间来存放该帧后续到来的数据
        if buf_len < frame_len {
            src.reserve(frame_len - buf_len);
            return Ok(None);
        }

        // 数据量足够了，从buf中取出数据转编成帧，并转换为指定类型后返回
        // 需同时将buf截断(split_to会截断)
        let frame_bytes = src.split_to(frame_len);
        // 先读取帧首，获得声明的帧中实际数据大小
        let mut msg_id_bytes = [0u8; 4];
        msg_id_bytes.copy_from_slice(&frame_bytes[4..8]);
        let msg_id = u32::from_be_bytes(msg_id_bytes) as usize;
        Ok(Some(MessagePacket {
            msg_id: msg_id as i32,
            buf: frame_bytes[8..].to_vec(),
        }))
    }
}

// 自定义的消息包协议
// #[derive(Debug, Clone)]
// pub struct MessagePacket {
//     pub msg_id: i32,
//     pub buf: Vec<u8>,
// }

pub struct MessagePacketEncryptCodec {}

impl MessagePacketEncryptCodec {
    const MAX_SIZE: usize = 8 * 1024 * 1024 as usize;
    pub fn encode(
        &mut self,
        buf: &mut bytes::BytesMut,
        dst: &mut bytes::BytesMut,
        key: &str,
        iv: &str,
    ) -> Result<(), std::io::Error> {
        let data = &buf[4..];
        if let Ok(encrypt_data) = aes256_cbc_encrypt(data, key, iv) {
            let data_len = encrypt_data.len();
            if data_len > Self::MAX_SIZE {
                return Err(std::io::Error::other("frame is too large"));
            }
            dst.reserve(data_len + 4);
            // 1. 写入消息长度
            dst.put_u32(data_len as u32);
            // 2. 再将实际数据放入帧尾
            dst.extend_from_slice(&encrypt_data);
        }
        Ok(())
    }
}

impl MessagePacketEncryptCodec {
    pub fn decode(
        &mut self,
        src: &mut bytes::BytesMut,
        key: &str,
        iv: &str,
    ) -> Result<Option<bytes::BytesMut>, std::io::Error> {
        let buf_len = src.len();

        // 如果buf中的数据量连长度声明的大小都不足，则先跳过等待后面更多数据的到来
        if buf_len < 4 {
            return Ok(None);
        }

        // 先读取帧首，获得声明的帧中实际数据大小
        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(&src[..4]);
        let data_len = u32::from_be_bytes(length_bytes) as usize;
        if data_len > Self::MAX_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Frame of length {} is too large.", data_len),
            ));
        }

        // 帧的总长度为 4 + frame_len
        let frame_len = data_len + 4;

        // buf中数据量不够，跳过，并预先申请足够的空闲空间来存放该帧后续到来的数据
        if buf_len < frame_len {
            src.reserve(frame_len - buf_len);
            return Ok(None);
        }

        // 数据量足够了，从buf中取出数据转编成帧，并转换为指定类型后返回
        // 需同时将buf截断(split_to会截断)
        let frame_bytes = src.split_to(frame_len);
        // 先读取帧首，获得声明的帧中实际数据大小
        // let mut msg_id_bytes = [0u8; 4];
        // msg_id_bytes.copy_from_slice(&frame_bytes[4..8]);
        if let Ok(decrypt_data) = aes256_cbc_decrypt(&frame_bytes[4..], key, iv) {
            let mut dst = bytes::BytesMut::new();
            let data_len = decrypt_data.len();
            if data_len > Self::MAX_SIZE {
                return Err(std::io::Error::other("frame is too large"));
            }
            dst.reserve(data_len + 4);
            // 1. 写入消息长度
            dst.put_u32(data_len as u32);
            // 2. 再将实际数据放入帧尾
            dst.extend_from_slice(&decrypt_data);
            Ok(Some(dst))
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "AESDecryptFailed",
            ));
        }
    }
}
// #[test]
// fn encode_decode() {
//     let mut dst_buf = bytes::BytesMut::new();
//     let mut en = MessagePacketCodec {};
//     let msg = MessagePacket {
//         msg_id: -1,
//         buf: b"helloword".to_vec(),
//     };

//     en.encode(msg, &mut dst_buf);
//     let mut received_data: Vec<u8> = vec![0; 4096];
//     let mut src = bytes::BytesMut::new();
//     src.put_slice(&received_data);
//     en.decode(&mut src);
// }
