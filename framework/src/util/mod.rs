//!
//!
//!

use crypto::aes;
use crypto::aes::KeySize::KeySize256;
use crypto::blockmodes::PkcsPadding;
use crypto::buffer::{ReadBuffer, RefReadBuffer, RefWriteBuffer, WriteBuffer};
use crypto::symmetriccipher::SymmetricCipherError;

use rand::distr::Alphanumeric;
use rand::Rng;

#[allow(unused_macros)]
macro_rules! rwlock {
    ($lock_result:expr) => {{
        $lock_result.unwrap_or_else(|e| e.into_inner())
    }};
}

macro_rules! static_mut_ref {
    ($place:expr) => {
        unsafe { &mut (*(&raw mut $place)) }
    };
}

pub(crate) use static_mut_ref;
#[allow(unused_imports)]
pub(crate) use rwlock;

/// Encrypt a buffer with the given key and iv using AES256/CBC/Pkcs encryption.
/// 加密(data:加密数据；key：密钥（长度为32的字符串）；iv：偏移量（长度为16的字符串）)
pub fn aes256_cbc_encrypt(
    data: &[u8],
    key: &str,
    iv: &str,
) -> Result<Vec<u8>, SymmetricCipherError> {
    let key = string_to_fixed_array_32(key);
    let iv = string_to_fixed_array_16(iv);
    let mut encryptor = aes::cbc_encryptor(KeySize256, &key, &iv, PkcsPadding);

    let mut buffer = [0; 4096];
    let mut write_buffer = RefWriteBuffer::new(&mut buffer);
    let mut read_buffer = RefReadBuffer::new(data);
    let mut final_result = Vec::new();

    loop {
        let result = encryptor.encrypt(&mut read_buffer, &mut write_buffer, true)?;
        final_result.extend(
            write_buffer
                .take_read_buffer()
                .take_remaining()
                .iter()
                .map(|&i| i),
        );
        match result {
            crypto::buffer::BufferResult::BufferUnderflow => break,
            _ => continue,
        }
    }

    Ok(final_result)
}

/// Decrypt a buffer with the given key and iv using AES256/CBC/Pkcs encryption.
/// 解密(data:加密数据；key：密钥（长度为32的字符串）；iv：偏移量（长度为16的字符串）)
pub fn aes256_cbc_decrypt(
    data: &[u8],
    key: &str,
    iv: &str,
) -> Result<Vec<u8>, SymmetricCipherError> {
    let key = string_to_fixed_array_32(key);
    let iv = string_to_fixed_array_16(iv);

    let mut decryptor = aes::cbc_decryptor(KeySize256, &key, &iv, PkcsPadding);

    let mut buffer = [0; 4096];
    let mut write_buffer = RefWriteBuffer::new(&mut buffer);
    let mut read_buffer = RefReadBuffer::new(data);
    let mut final_result = Vec::new();

    loop {
        let result = decryptor.decrypt(&mut read_buffer, &mut write_buffer, true)?;
        final_result.extend(
            write_buffer
                .take_read_buffer()
                .take_remaining()
                .iter()
                .map(|&i| i),
        );
        match result {
            crypto::buffer::BufferResult::BufferUnderflow => break,
            _ => continue,
        }
    }

    Ok(final_result)
}

/// 将字符串转为[u8; 32]
fn string_to_fixed_array_32(s: &str) -> [u8; 32] {
    s.as_bytes().try_into().expect("字符串转为[u8; 32]失败")
}

/// 将字符串转为[u8; 16]
fn string_to_fixed_array_16(s: &str) -> [u8; 16] {
    s.as_bytes().try_into().expect("字符串转为[u8; 16]失败")
}

// 生成随机字符串
pub fn generate_random_string(length: usize) -> String {
    let s: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect();
    s
}

#[test]
fn test_aes256_cbc() {
    // 设置key长度为32
    let key = "abcdefghijklmnopqrstuvwxyz123456";
    // 设置iv长度为16
    let iv = "1234567890123456";
    // 设置需要加密的字符串
    let data = "Hello, world!";
    // 加密操作
    let encrypted_data = aes256_cbc_encrypt(data.as_bytes(), &key, &iv).unwrap();
    // 解密操作
    let decrypted_data = aes256_cbc_decrypt(encrypted_data.as_slice(), &key, &iv).unwrap();
    // 转为原始字符串信息
    let result = std::str::from_utf8(decrypted_data.as_slice()).unwrap();

    assert_eq!(data, result);
    println!("{}", result);
}
