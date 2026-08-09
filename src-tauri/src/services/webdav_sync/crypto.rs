use std::collections::HashMap;
use std::sync::Arc;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose, Engine as _};
use chacha20poly1305::aead::{Aead, AeadInPlace, Payload};
use chacha20poly1305::{KeyInit, Tag, XChaCha20Poly1305, XNonce};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroize;

pub const CONFIG_PATH: &str = ".qc-e2ee.json";

const CONFIG_FORMAT: &str = "qc-e2ee-config-v1";
const DATA_FORMAT: &str = "qc-e2ee-data-v1";
const FILE_MAGIC: &[u8; 8] = b"QCFE2EE1";
const FILE_FRAME_AAD_PREFIX: &[u8] = b"qc-e2ee-file-frame-v1";
const CIPHER_NAME: &str = "xchacha20poly1305";
const KDF_NAME: &str = "argon2id";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const TAG_LEN: u64 = 16;
const FILE_HEADER_LEN: u64 = 24;
const FILE_FRAME_HEADER_LEN: u64 = 28;
const MAX_FILE_CHUNK_SIZE: usize = 64 * 1024 * 1024;
const DEFAULT_MEMORY_KIB: u32 = 64 * 1024;
const DEFAULT_ITERATIONS: u32 = 3;
const DEFAULT_PARALLELISM: u32 = 1;

static MASTER_KEY_CACHE: Lazy<Mutex<HashMap<String, Arc<MasterKey>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static CONFIG_CACHE: Lazy<Mutex<HashMap<String, WebdavE2eeConfig>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebdavE2eeConfig {
    pub format: String,
    pub kdf: KdfConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KdfConfig {
    pub name: String,
    pub salt: String,
    #[serde(rename = "memoryKiB")]
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

#[derive(Clone)]
pub struct WebdavCryptoContext {
    key: Arc<MasterKey>,
}

impl std::fmt::Debug for WebdavCryptoContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不打印密钥内容
        f.debug_struct("WebdavCryptoContext")
            .field("key", &"<redacted>")
            .finish()
    }
}

struct MasterKey {
    bytes: [u8; KEY_LEN],
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
struct DataEnvelope {
    format: String,
    cipher: CipherEnvelope,
    payload: String,
}

#[derive(Serialize, Deserialize)]
struct CipherEnvelope {
    name: String,
    nonce: String,
}

pub fn create_config() -> WebdavE2eeConfig {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    WebdavE2eeConfig {
        format: CONFIG_FORMAT.to_string(),
        kdf: KdfConfig {
            name: KDF_NAME.to_string(),
            salt: general_purpose::STANDARD.encode(salt),
            memory_kib: DEFAULT_MEMORY_KIB,
            iterations: DEFAULT_ITERATIONS,
            parallelism: DEFAULT_PARALLELISM,
        },
    }
}

pub fn context_for_config(
    scope: &str,
    config: &WebdavE2eeConfig,
    password: &str,
) -> Result<WebdavCryptoContext, String> {
    validate_config(config)?;
    if password.is_empty() {
        return Err("请先设置 WebDAV 云端加密密码".to_string());
    }

    let cache_key = cache_key(scope, config, password);
    if let Some(key) = MASTER_KEY_CACHE.lock().get(&cache_key).cloned() {
        return Ok(WebdavCryptoContext { key });
    }

    let key = Arc::new(MasterKey {
        bytes: derive_master_key(config, password)?,
    });
    MASTER_KEY_CACHE.lock().insert(cache_key, key.clone());
    Ok(WebdavCryptoContext { key })
}

pub fn cached_config(scope: &str) -> Option<WebdavE2eeConfig> {
    CONFIG_CACHE.lock().get(scope).cloned()
}

pub fn cache_config(scope: &str, config: &WebdavE2eeConfig) {
    CONFIG_CACHE.lock().insert(scope.to_string(), config.clone());
}

pub fn clear_cached_keys() {
    MASTER_KEY_CACHE.lock().clear();
    CONFIG_CACHE.lock().clear();
}

impl WebdavCryptoContext {
    pub fn encrypted_file_size(&self, plain_size: u64, chunk_size: usize) -> Result<u64, String> {
        validate_file_chunk_size(chunk_size)?;
        let chunk_size = chunk_size as u64;
        let frames = if plain_size == 0 {
            0
        } else {
            plain_size
                .checked_add(chunk_size - 1)
                .ok_or_else(|| "云端文件过大".to_string())?
                / chunk_size
        };
        FILE_HEADER_LEN
            .checked_add(plain_size)
            .and_then(|value| value.checked_add(frames.checked_mul(FILE_FRAME_HEADER_LEN + TAG_LEN)?))
            .ok_or_else(|| "云端加密文件大小溢出".to_string())
    }

    pub async fn write_encrypted_file<R, W>(
        &self,
        path: &str,
        mut reader: R,
        mut writer: W,
        plain_size: u64,
        chunk_size: usize,
        progress: Option<Arc<dyn Fn(u64) + Send + Sync + 'static>>,
    ) -> Result<String, String>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        validate_file_chunk_size(chunk_size)?;
        if let Some(callback) = progress.as_ref() {
            callback(0);
        }

        writer
            .write_all(FILE_MAGIC)
            .await
            .map_err(|e| format!("写入云端加密文件头失败: {}", e))?;
        writer
            .write_all(&(chunk_size as u64).to_le_bytes())
            .await
            .map_err(|e| format!("写入云端加密文件头失败: {}", e))?;
        writer
            .write_all(&plain_size.to_le_bytes())
            .await
            .map_err(|e| format!("写入云端加密文件头失败: {}", e))?;

        let cipher = XChaCha20Poly1305::new_from_slice(&self.key.bytes)
            .map_err(|e| format!("初始化 WebDAV 加密器失败: {}", e))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; chunk_size];
        let mut sent_bytes = 0u64;
        let mut frame_index = 0u64;

        loop {
            let read = read_file_chunk(&mut reader, &mut buffer).await?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            let plain_len = u32::try_from(read).map_err(|_| "云端文件分片过大".to_string())?;
            let mut nonce = [0u8; NONCE_LEN];
            OsRng.fill_bytes(&mut nonce);
            let tag = cipher
                .encrypt_in_place_detached(
                    XNonce::from_slice(&nonce),
                    &file_frame_aad(path, frame_index, plain_len),
                    &mut buffer[..read],
                )
                .map_err(|_| "加密云端文件分片失败".to_string())?;

            writer
                .write_all(&plain_len.to_le_bytes())
                .await
                .map_err(|e| format!("写入云端加密文件分片失败: {}", e))?;
            writer
                .write_all(&nonce)
                .await
                .map_err(|e| format!("写入云端加密文件分片失败: {}", e))?;
            writer
                .write_all(&buffer[..read])
                .await
                .map_err(|e| format!("写入云端加密文件分片失败: {}", e))?;
            writer
                .write_all(&tag)
                .await
                .map_err(|e| format!("写入云端加密文件分片失败: {}", e))?;

            sent_bytes = sent_bytes.saturating_add(read as u64);
            if sent_bytes > plain_size {
                return Err("待上传文件大小发生变化".to_string());
            }
            if let Some(callback) = progress.as_ref() {
                callback(sent_bytes);
            }
            frame_index = frame_index
                .checked_add(1)
                .ok_or_else(|| "云端文件分片数量过多".to_string())?;
        }

        if sent_bytes != plain_size {
            return Err("待上传文件大小发生变化".to_string());
        }
        writer
            .shutdown()
            .await
            .map_err(|e| format!("结束云端加密上传流失败: {}", e))?;
        Ok(hex::encode(hasher.finalize()))
    }

    pub async fn read_encrypted_file<R, W>(
        &self,
        path: &str,
        mut reader: R,
        mut writer: W,
    ) -> Result<String, String>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut magic = [0u8; FILE_MAGIC.len()];
        reader
            .read_exact(&mut magic)
            .await
            .map_err(|e| format!("读取云端加密文件头失败: {}", e))?;
        if &magic != FILE_MAGIC {
            return Err("云端文件加密格式不兼容".to_string());
        }

        let mut u64_bytes = [0u8; 8];
        reader
            .read_exact(&mut u64_bytes)
            .await
            .map_err(|e| format!("读取云端加密文件头失败: {}", e))?;
        let chunk_size = u64::from_le_bytes(u64_bytes);
        let chunk_size_usize = usize::try_from(chunk_size).map_err(|_| "云端文件分片大小无效".to_string())?;
        validate_file_chunk_size(chunk_size_usize)?;

        reader
            .read_exact(&mut u64_bytes)
            .await
            .map_err(|e| format!("读取云端加密文件头失败: {}", e))?;
        let plain_size = u64::from_le_bytes(u64_bytes);

        let cipher = XChaCha20Poly1305::new_from_slice(&self.key.bytes)
            .map_err(|e| format!("初始化 WebDAV 解密器失败: {}", e))?;
        let mut hasher = Sha256::new();
        let mut remaining = plain_size;
        let mut frame_index = 0u64;
        let mut encrypted = vec![0u8; chunk_size_usize + TAG_LEN as usize];
        while remaining > 0 {
            let mut len_bytes = [0u8; 4];
            reader
                .read_exact(&mut len_bytes)
                .await
                .map_err(|e| format!("读取云端加密文件分片失败: {}", e))?;
            let plain_len = u32::from_le_bytes(len_bytes);
            if plain_len == 0 || plain_len as u64 > remaining || plain_len as usize > chunk_size_usize {
                return Err("云端加密文件分片长度无效".to_string());
            }

            let mut nonce = [0u8; NONCE_LEN];
            reader
                .read_exact(&mut nonce)
                .await
                .map_err(|e| format!("读取云端加密文件分片失败: {}", e))?;
            let encrypted_len = plain_len as usize + TAG_LEN as usize;
            reader
                .read_exact(&mut encrypted[..encrypted_len])
                .await
                .map_err(|e| format!("读取云端加密文件分片失败: {}", e))?;

            let (ciphertext, tag) = encrypted[..encrypted_len].split_at_mut(plain_len as usize);
            cipher
                .decrypt_in_place_detached(
                    XNonce::from_slice(&nonce),
                    &file_frame_aad(path, frame_index, plain_len),
                    ciphertext,
                    Tag::from_slice(tag),
                )
                .map_err(|_| "WebDAV 云端文件解密失败，请检查云端加密密码".to_string())?;
            if ciphertext.len() != plain_len as usize {
                return Err("云端文件解密长度异常".to_string());
            }
            hasher.update(&ciphertext[..]);
            writer
                .write_all(&ciphertext[..])
                .await
                .map_err(|e| format!("写入本地下载文件失败: {}", e))?;

            remaining -= plain_len as u64;
            frame_index = frame_index
                .checked_add(1)
                .ok_or_else(|| "云端文件分片数量过多".to_string())?;
        }

        writer
            .flush()
            .await
            .map_err(|e| format!("写入本地下载文件失败: {}", e))?;
        Ok(hex::encode(hasher.finalize()))
    }

    pub fn encrypt_bytes(&self, path: &str, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new_from_slice(&self.key.bytes)
            .map_err(|e| format!("初始化 WebDAV 加密器失败: {}", e))?;
        let payload = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: path.as_bytes(),
                },
            )
            .map_err(|e| format!("加密 WebDAV 数据失败: {}", e))?;
        let envelope = DataEnvelope {
            format: DATA_FORMAT.to_string(),
            cipher: CipherEnvelope {
                name: CIPHER_NAME.to_string(),
                nonce: general_purpose::STANDARD.encode(nonce),
            },
            payload: general_purpose::STANDARD.encode(payload),
        };
        serde_json::to_vec(&envelope).map_err(|e| format!("编码 WebDAV 加密信封失败: {}", e))
    }

    pub fn decrypt_bytes(&self, path: &str, encrypted: &[u8]) -> Result<Vec<u8>, String> {
        let envelope: DataEnvelope = serde_json::from_slice(encrypted)
            .map_err(|_| "WebDAV 数据不是 QuickClipboard 加密格式".to_string())?;
        if envelope.format != DATA_FORMAT {
            return Err("WebDAV 数据加密格式不兼容".to_string());
        }
        if envelope.cipher.name != CIPHER_NAME {
            return Err("WebDAV 数据加密算法不受支持".to_string());
        }
        let nonce = general_purpose::STANDARD
            .decode(envelope.cipher.nonce)
            .map_err(|e| format!("解析 WebDAV 加密 nonce 失败: {}", e))?;
        if nonce.len() != NONCE_LEN {
            return Err("WebDAV 加密 nonce 长度无效".to_string());
        }
        let payload = general_purpose::STANDARD
            .decode(envelope.payload)
            .map_err(|e| format!("解析 WebDAV 加密数据失败: {}", e))?;
        let cipher = XChaCha20Poly1305::new_from_slice(&self.key.bytes)
            .map_err(|e| format!("初始化 WebDAV 解密器失败: {}", e))?;
        cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &payload,
                    aad: path.as_bytes(),
                },
            )
            .map_err(|_| "WebDAV 数据解密失败，请检查云端加密密码".to_string())
    }
}

fn validate_config(config: &WebdavE2eeConfig) -> Result<(), String> {
    if config.format != CONFIG_FORMAT {
        return Err("WebDAV 云端加密配置格式不兼容".to_string());
    }
    if config.kdf.name != KDF_NAME {
        return Err("WebDAV 云端加密 KDF 不受支持".to_string());
    }
    if config.kdf.memory_kib == 0 || config.kdf.iterations == 0 || config.kdf.parallelism == 0 {
        return Err("WebDAV 云端加密 KDF 参数无效".to_string());
    }
    Ok(())
}

fn derive_master_key(config: &WebdavE2eeConfig, password: &str) -> Result<[u8; KEY_LEN], String> {
    let salt = general_purpose::STANDARD
        .decode(&config.kdf.salt)
        .map_err(|e| format!("解析 WebDAV 云端加密 salt 失败: {}", e))?;
    let params = Params::new(
        config.kdf.memory_kib,
        config.kdf.iterations,
        config.kdf.parallelism,
        Some(KEY_LEN),
    )
    .map_err(|e| format!("WebDAV 云端加密 KDF 参数无效: {}", e))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(password.as_bytes(), &salt, &mut key)
        .map_err(|e| format!("派生 WebDAV 云端加密密钥失败: {}", e))?;
    Ok(key)
}

fn cache_key(scope: &str, config: &WebdavE2eeConfig, password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    format!("{}\n{}\n{}", scope, config.kdf.salt, hex::encode(hasher.finalize()))
}

fn validate_file_chunk_size(chunk_size: usize) -> Result<(), String> {
    if chunk_size == 0 || chunk_size > MAX_FILE_CHUNK_SIZE {
        return Err("云端文件分片大小无效".to_string());
    }
    Ok(())
}

fn file_frame_aad(path: &str, frame_index: u64, plain_len: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(FILE_FRAME_AAD_PREFIX.len() + path.len() + 20);
    out.extend_from_slice(FILE_FRAME_AAD_PREFIX);
    out.extend_from_slice(path.as_bytes());
    out.extend_from_slice(&frame_index.to_le_bytes());
    out.extend_from_slice(&plain_len.to_le_bytes());
    out
}

async fn read_file_chunk<R>(reader: &mut R, buffer: &mut [u8]) -> Result<usize, String>
where
    R: AsyncRead + Unpin,
{
    let mut read_total = 0usize;
    while read_total < buffer.len() {
        let read = reader
            .read(&mut buffer[read_total..])
            .await
            .map_err(|e| format!("读取待上传文件失败: {}", e))?;
        if read == 0 {
            break;
        }
        read_total = read_total.saturating_add(read);
    }
    Ok(read_total)
}

#[cfg(test)]
mod tests {
    use super::{
        cache_config, cache_key, cached_config, clear_cached_keys, context_for_config, create_config,
        file_frame_aad, validate_config, CIPHER_NAME, DATA_FORMAT, FILE_FRAME_AAD_PREFIX, FILE_MAGIC,
        WebdavCryptoContext,
    };
    use base64::{engine::general_purpose, Engine as _};
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;

    fn test_context() -> WebdavCryptoContext {
        let config = create_config();
        context_for_config("test", &config, "secret").unwrap()
    }

    #[test]
    fn encrypts_and_decrypts_webdav_payload() {
        clear_cached_keys();
        let config = create_config();
        let context = context_for_config("test", &config, "secret").unwrap();
        let encrypted = context.encrypt_bytes("history/index.json", b"hello").unwrap();
        assert_ne!(encrypted, b"hello");
        let decrypted = context.decrypt_bytes("history/index.json", &encrypted).unwrap();
        assert_eq!(decrypted, b"hello");
    }

    #[test]
    fn rejects_payload_moved_to_another_path() {
        clear_cached_keys();
        let config = create_config();
        let context = context_for_config("test", &config, "secret").unwrap();
        let encrypted = context.encrypt_bytes("history/index.json", b"hello").unwrap();
        let result = context.decrypt_bytes("favorites/index.json", &encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn create_config_has_expected_invariants() {
        let config = create_config();
        assert_eq!(config.format, "qc-e2ee-config-v1");
        assert_eq!(config.kdf.name, "argon2id");
        assert_eq!(config.kdf.memory_kib, 64 * 1024);
        assert_eq!(config.kdf.iterations, 3);
        assert_eq!(config.kdf.parallelism, 1);
        // 16 random bytes -> 24 base64 chars
        assert_eq!(config.kdf.salt.len(), 24);
        let salt = general_purpose::STANDARD
            .decode(&config.kdf.salt)
            .expect("salt must be valid base64");
        assert_eq!(salt.len(), 16);
        // 每次创建随机 salt，两次创建互不相同
        let other = create_config();
        assert_ne!(config.kdf.salt, other.kdf.salt);
    }

    #[test]
    fn context_rejects_foreign_config_format() {
        let mut config = create_config();
        config.format = "other-format".to_string();
        let err = context_for_config("test", &config, "secret").unwrap_err();
        assert!(err.contains("WebDAV 云端加密配置格式不兼容"), "got: {}", err);
    }

    #[test]
    fn context_rejects_unsupported_kdf() {
        let mut config = create_config();
        config.kdf.name = "scrypt".to_string();
        let err = context_for_config("test", &config, "secret").unwrap_err();
        assert!(err.contains("WebDAV 云端加密 KDF 不受支持"), "got: {}", err);
    }

    #[test]
    fn context_rejects_zero_kdf_parameters() {
        let mut config = create_config();
        config.kdf.memory_kib = 0;
        let err = context_for_config("test", &config, "secret").unwrap_err();
        assert!(err.contains("WebDAV 云端加密 KDF 参数无效"), "got: {}", err);

        let mut config = create_config();
        config.kdf.iterations = 0;
        let err = context_for_config("test", &config, "secret").unwrap_err();
        assert!(err.contains("WebDAV 云端加密 KDF 参数无效"), "got: {}", err);

        let mut config = create_config();
        config.kdf.parallelism = 0;
        let err = context_for_config("test", &config, "secret").unwrap_err();
        assert!(err.contains("WebDAV 云端加密 KDF 参数无效"), "got: {}", err);
    }

    #[test]
    fn context_rejects_empty_password() {
        let config = create_config();
        let err = context_for_config("test", &config, "").unwrap_err();
        assert_eq!(err, "请先设置 WebDAV 云端加密密码");
    }

    #[test]
    fn validate_config_accepts_created_config() {
        let config = create_config();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn key_derivation_is_deterministic_across_contexts() {
        clear_cached_keys();
        let config = create_config();
        let first = context_for_config("test", &config, "secret").unwrap();
        let second = context_for_config("test", &config, "secret").unwrap();
        let encrypted = first.encrypt_bytes("history/index.json", b"payload").unwrap();
        // 同配置同密码的第二个上下文必须能解开
        assert_eq!(
            second.decrypt_bytes("history/index.json", &encrypted).unwrap(),
            b"payload"
        );
    }

    #[test]
    fn wrong_password_cannot_decrypt() {
        clear_cached_keys();
        let config = create_config();
        let context = context_for_config("test", &config, "secret").unwrap();
        let encrypted = context.encrypt_bytes("history/index.json", b"payload").unwrap();
        let wrong = context_for_config("test", &config, "wrong").unwrap();
        let err = wrong.decrypt_bytes("history/index.json", &encrypted).unwrap_err();
        assert!(err.contains("WebDAV 数据解密失败，请检查云端加密密码"), "got: {}", err);
    }

    #[test]
    fn cache_config_roundtrip_and_clear() {
        clear_cached_keys();
        let config = create_config();
        assert!(cached_config("scope-a").is_none());
        cache_config("scope-a", &config);
        let cached = cached_config("scope-a").expect("cached config must be readable");
        assert_eq!(cached.kdf.salt, config.kdf.salt);
        assert!(cached_config("scope-b").is_none());
        clear_cached_keys();
        assert!(cached_config("scope-a").is_none());
    }

    #[test]
    fn cache_key_is_deterministic_and_sensitive_to_inputs() {
        let config = create_config();
        let first = cache_key("scope", &config, "pw");
        let again = cache_key("scope", &config, "pw");
        assert_eq!(first, again);
        assert_ne!(first, cache_key("scope", &config, "other"));
        assert_ne!(first, cache_key("other-scope", &config, "pw"));
    }

    #[test]
    fn encrypted_file_size_math_is_exact() {
        let context = test_context();
        // header 24 字节 + 明文 + 分片数 * (帧头 28 + tag 16)
        assert_eq!(context.encrypted_file_size(0, 1024).unwrap(), 24);
        assert_eq!(context.encrypted_file_size(1, 1024).unwrap(), 24 + 1 + 44);
        assert_eq!(context.encrypted_file_size(1024, 1024).unwrap(), 24 + 1024 + 44);
        assert_eq!(context.encrypted_file_size(1025, 1024).unwrap(), 24 + 1025 + 88);
        assert_eq!(context.encrypted_file_size(10000, 1024).unwrap(), 24 + 10000 + 440);
        assert_eq!(context.encrypted_file_size(64 * 1024 * 1024, 64 * 1024 * 1024).unwrap(), 24 + 64 * 1024 * 1024 + 44);
    }

    #[test]
    fn encrypted_file_size_rejects_invalid_chunk_sizes() {
        let context = test_context();
        let err = context.encrypted_file_size(100, 0).unwrap_err();
        assert!(err.contains("云端文件分片大小无效"), "got: {}", err);
        let err = context.encrypted_file_size(100, 64 * 1024 * 1024 + 1).unwrap_err();
        assert!(err.contains("云端文件分片大小无效"), "got: {}", err);
    }

    #[test]
    fn encrypted_file_size_overflow_errors() {
        let context = test_context();
        // 明文 + chunk-1 本身溢出 -> 云端文件过大
        let err = context.encrypted_file_size(u64::MAX, 2).unwrap_err();
        assert_eq!(err, "云端文件过大");
        // 帧数 * 帧开销溢出 -> 云端加密文件大小溢出
        let err = context.encrypted_file_size(u64::MAX - 1, 2).unwrap_err();
        assert!(err.contains("云端加密文件大小溢出"), "got: {}", err);
    }

    #[test]
    fn file_frame_aad_layout_is_stable() {
        // wire 字节字面量：前缀 + 路径 + frame_index(le u64) + plain_len(le u32)
        assert_eq!(
            file_frame_aad("cloud_files/objects/x.qcf", 7, 3),
            b"qc-e2ee-file-frame-v1cloud_files/objects/x.qcf\x07\x00\x00\x00\x00\x00\x00\x00\x03\x00\x00\x00"
        );
    }

    #[test]
    fn bytes_envelope_is_json_with_expected_fields() {
        let context = test_context();
        let encrypted = context.encrypt_bytes("history/index.json", b"hello").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&encrypted).expect("envelope must be JSON");
        assert_eq!(value["format"], "qc-e2ee-data-v1");
        assert_eq!(value["cipher"]["name"], "xchacha20poly1305");
        let nonce = value["cipher"]["nonce"].as_str().expect("nonce missing");
        assert_eq!(nonce.len(), 32); // base64(24)
        // 明文 5 字节 + tag 16 字节 = 21 字节 -> base64 28 字符
        assert_eq!(value["payload"].as_str().expect("payload missing").len(), 28);
        assert_eq!(general_purpose::STANDARD.decode(value["payload"].as_str().unwrap()).unwrap().len(), 21);
    }

    #[test]
    fn bytes_reject_tampered_payload() {
        let context = test_context();
        let mut encrypted = context.encrypt_bytes("history/index.json", b"hello").unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&encrypted).unwrap();
        let mut payload = general_purpose::STANDARD
            .decode(value["payload"].as_str().unwrap())
            .unwrap();
        payload[0] ^= 0x01;
        value["payload"] = serde_json::Value::String(general_purpose::STANDARD.encode(payload));
        encrypted = serde_json::to_vec(&value).unwrap();
        let err = context.decrypt_bytes("history/index.json", &encrypted).unwrap_err();
        assert!(err.contains("WebDAV 数据解密失败，请检查云端加密密码"), "got: {}", err);
    }

    #[test]
    fn bytes_reject_foreign_format_and_cipher() {
        let context = test_context();
        let encrypted = context.encrypt_bytes("history/index.json", b"hello").unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&encrypted).unwrap();
        value["format"] = serde_json::Value::String("other".to_string());
        let err = context.decrypt_bytes("history/index.json", &serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(err.contains("WebDAV 数据加密格式不兼容"), "got: {}", err);

        let encrypted = context.encrypt_bytes("history/index.json", b"hello").unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&encrypted).unwrap();
        value["cipher"]["name"] = serde_json::Value::String("aes-gcm".to_string());
        let err = context.decrypt_bytes("history/index.json", &serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(err.contains("WebDAV 数据加密算法不受支持"), "got: {}", err);
    }

    #[test]
    fn bytes_reject_bad_nonce_and_non_envelope() {
        let context = test_context();
        let value = serde_json::json!({
            "format": DATA_FORMAT,
            "cipher": { "name": CIPHER_NAME, "nonce": general_purpose::STANDARD.encode([0u8; 4]) },
            "payload": general_purpose::STANDARD.encode([0u8; 8]),
        });
        let err = context
            .decrypt_bytes("history/index.json", &serde_json::to_vec(&value).unwrap())
            .unwrap_err();
        assert!(err.contains("WebDAV 加密 nonce 长度无效"), "got: {}", err);

        let err = context.decrypt_bytes("history/index.json", b"not json at all").unwrap_err();
        assert!(err.contains("WebDAV 数据不是 QuickClipboard 加密格式"), "got: {}", err);
    }

    #[tokio::test]
    async fn encrypted_file_stream_returns_plaintext_sha256() {
        clear_cached_keys();
        let context = test_context();
        let plaintext = (0..10000).map(|value| (value % 251) as u8).collect::<Vec<_>>();
        let mut expected_hasher = Sha256::new();
        expected_hasher.update(&plaintext);
        let expected_sha = hex::encode(expected_hasher.finalize());

        let (encrypted_writer, encrypted_reader) = tokio::io::duplex(4096);
        let write_context = context.clone();
        let plain_for_write = plaintext.clone();
        let write_task = tokio::spawn(async move {
            let reader = std::io::Cursor::new(plain_for_write);
            write_context
                .write_encrypted_file("cloud_files/objects/a.qcf", reader, encrypted_writer, 10000, 1024, None)
                .await
        });

        let (decrypted_writer, mut decrypted_reader) = tokio::io::duplex(4096);
        let read_context = context.clone();
        let read_task = tokio::spawn(async move {
            read_context
                .read_encrypted_file("cloud_files/objects/a.qcf", encrypted_reader, decrypted_writer)
                .await
        });

        let mut out = Vec::new();
        decrypted_reader.read_to_end(&mut out).await.unwrap();
        let returned_sha = write_task.await.unwrap().unwrap();
        let read_sha = read_task.await.unwrap().unwrap();
        assert_eq!(out, plaintext);
        assert_eq!(returned_sha, expected_sha);
        assert_eq!(read_sha, expected_sha);
    }

    #[tokio::test]
    async fn encrypted_file_stream_rejects_bad_magic() {
        clear_cached_keys();
        let context = test_context();
        let mut bogus = Vec::new();
        bogus.extend_from_slice(b"BADMAGIC");
        bogus.extend_from_slice(&1024u64.to_le_bytes());
        bogus.extend_from_slice(&10u64.to_le_bytes());
        let mut sink = Vec::new();
        let err = context
            .read_encrypted_file("cloud_files/objects/a.qcf", std::io::Cursor::new(bogus), &mut sink)
            .await
            .unwrap_err();
        assert!(err.contains("云端文件加密格式不兼容"), "got: {}", err);
    }

    #[tokio::test]
    async fn encrypted_file_stream_rejects_plain_size_mismatch() {
        clear_cached_keys();
        let context = test_context();
        let plaintext = vec![7u8; 10000];
        // 声明 5000 字节但实际读出 10000 -> 中途报错
        let mut sink = Vec::new();
        let write_context = context.clone();
        let result = write_context
            .write_encrypted_file("cloud_files/objects/a.qcf", std::io::Cursor::new(plaintext.clone()), &mut sink, 5000, 1024, None)
            .await;
        let err = result.unwrap_err();
        assert!(err.contains("待上传文件大小发生变化"), "got: {}", err);
        // 声明 10000 但实际 5000 -> 收尾校验失败
        let mut sink = Vec::new();
        let write_context = context.clone();
        let result = write_context
            .write_encrypted_file("cloud_files/objects/a.qcf", std::io::Cursor::new(plaintext[..5000].to_vec()), &mut sink, 10000, 1024, None)
            .await;
        let err = result.unwrap_err();
        assert!(err.contains("待上传文件大小发生变化"), "got: {}", err);
    }

    #[tokio::test]
    async fn encrypted_file_stream_rejects_invalid_frame_length() {
        clear_cached_keys();
        let context = test_context();
        // 手工构造：合法头 + 帧长度 0（明文声明 10 字节但没有任何有效帧）
        let mut bogus = Vec::new();
        bogus.extend_from_slice(FILE_MAGIC);
        bogus.extend_from_slice(&1024u64.to_le_bytes());
        bogus.extend_from_slice(&10u64.to_le_bytes());
        bogus.extend_from_slice(&0u32.to_le_bytes()); // plain_len == 0 非法
        let mut sink = Vec::new();
        let err = context
            .read_encrypted_file("cloud_files/objects/a.qcf", std::io::Cursor::new(bogus), &mut sink)
            .await
            .unwrap_err();
        assert!(err.contains("云端加密文件分片长度无效"), "got: {}", err);
    }

    #[tokio::test]
    async fn encrypted_file_stream_rejects_tampered_frame() {
        clear_cached_keys();
        let context = test_context();
        let plaintext = (0..10000).map(|value| (value % 251) as u8).collect::<Vec<_>>();
        let mut encrypted = Vec::new();
        let write_context = context.clone();
        let plain_for_write = plaintext.clone();
        write_context
            .write_encrypted_file(
                "cloud_files/objects/a.qcf",
                std::io::Cursor::new(plain_for_write),
                &mut encrypted,
                10000,
                1024,
                None,
            )
            .await
            .unwrap();
        // 第一帧密文从偏移 52 开始（头 24 + 帧长 4 + nonce 24），翻转其中一字节
        encrypted[52 + 100] ^= 0x01;
        let mut sink = Vec::new();
        let err = context
            .read_encrypted_file("cloud_files/objects/a.qcf", std::io::Cursor::new(encrypted), &mut sink)
            .await
            .unwrap_err();
        assert!(err.contains("WebDAV 云端文件解密失败，请检查云端加密密码"), "got: {}", err);
    }

    #[tokio::test]
    async fn streams_encrypted_file_frames() {
        clear_cached_keys();
        let config = create_config();
        let context = context_for_config("test", &config, "secret").unwrap();
        let path = "cloud_files/objects/test.qcf";
        let plaintext = (0..10000).map(|value| (value % 251) as u8).collect::<Vec<_>>();
        let plain_len = plaintext.len() as u64;
        let plain_for_task = plaintext.clone();

        let (encrypted_writer, encrypted_reader) = tokio::io::duplex(4096);
        let write_context = context.clone();
        let write_task = tokio::spawn(async move {
            let reader = std::io::Cursor::new(plain_for_task);
            write_context
                .write_encrypted_file(path, reader, encrypted_writer, plain_len, 1024, None)
                .await
        });

        let (decrypted_writer, mut decrypted_reader) = tokio::io::duplex(4096);
        let read_context = context.clone();
        let read_task = tokio::spawn(async move {
            read_context
                .read_encrypted_file(path, encrypted_reader, decrypted_writer)
                .await
        });

        let mut out = Vec::new();
        decrypted_reader.read_to_end(&mut out).await.unwrap();
        write_task.await.unwrap().unwrap();
        read_task.await.unwrap().unwrap();
        assert_eq!(out, plaintext);
    }
}
