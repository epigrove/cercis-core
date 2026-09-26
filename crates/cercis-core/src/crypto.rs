//! 加密原语（S11 加密共享的底座）。
//!
//! 内核**不发明算法**，只用成熟实现（RustCrypto / dalek）拼出四件事：
//!
//! | 用途 | 算法 | 说明 |
//! |---|---|---|
//! | 口令 → 密钥 | **Argon2id** | 抗暴力破解；参数随密钥文件一起存，将来可升级 |
//! | 内容加解密 | **ChaCha20-Poly1305** | AEAD：密文自带完整性校验，改一个字节就解不开 |
//! | 密钥封装 | **X25519 + HKDF-SHA256 + AEAD** | 把区域内容密钥单独发给某个成员（公钥封装） |
//! | 随机数 / 指纹 | `OsRng` / SHA-256 | 随机数来自操作系统 |
//!
//! ## 为什么用 ChaCha20-Poly1305 而不是 AES-GCM
//! 它不依赖 CPU 的 AES 指令，在手机、老机器、嵌入式上速度稳定；
//! 且是 IETF 标准（RFC 8439），各家库互相兼容。
//!
//! ## 确定性随机数（重要）
//! 文件加密用的 nonce 不随机取，而是 `SHA256(域分隔 || 密钥 || 明文)` 前 12 字节。
//! 好处：**同一份明文 + 同一把密钥 → 同一份密文**，于是
//! - 流水里的指纹可对账（同一次写入重放得到同一密文）；
//! - 内容寻址的历史对象库能去重；
//! - 备份/NAS 同步不会因为"每次密文都不一样"而反复冲突。
//! 安全性：nonce 复用只会发生在**明文完全相同**时（此时密文本就该相同），
//! 不同明文撞 nonce 的概率是 2^-96 级别。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::{CercisError, Result};

/// 对称密钥长度（字节）。
pub const KEY_LEN: usize = 32;
/// AEAD nonce 长度（字节）。
pub const NONCE_LEN: usize = 12;
/// 口令派生用的盐长度（字节）。
pub const SALT_LEN: usize = 16;
/// 内容加密算法标识（写进文件头与项目声明，便于将来换算法）。
pub const AEAD_ALG: &str = "chacha20poly1305";
/// 口令派生算法标识。
pub const KDF_ALG: &str = "argon2id";

/// 密钥指纹：取 SHA-256 前 4 字节（8 个十六进制字符）。
///
/// 用于人工核对（"这把密钥对不对"）与日志对账；**不能**当密钥用。
pub fn fingerprint(key: &[u8]) -> String {
    let digest = Sha256::digest(key);
    hex::encode(&digest[..4])
}

/// 口令派生参数（Argon2id）。
///
/// 存在密钥文件里：将来调强参数时，老密钥文件仍能按自己的参数解开。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// 默认强度：64 MiB 内存、3 轮、单线程（约 0.1–0.3 秒）。
    pub fn strong() -> Self {
        Self {
            m_cost: 64 * 1024,
            t_cost: 3,
            p_cost: 1,
        }
    }

    /// 轻量档：测试与低配设备用（**不要**用于真实敏感数据）。
    pub fn light() -> Self {
        Self {
            m_cost: 8 * 1024,
            t_cost: 1,
            p_cost: 1,
        }
    }

    /// 按环境变量选档：`CERCIS_SHARE_KDF=light` 走轻量档（测试/CI），否则默认强度。
    pub fn from_env() -> Self {
        match std::env::var("CERCIS_SHARE_KDF") {
            Ok(v) if v.eq_ignore_ascii_case("light") || v.eq_ignore_ascii_case("test") => {
                Self::light()
            }
            _ => Self::strong(),
        }
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::strong()
    }
}

/// 取操作系统随机数。
pub fn random_bytes(n: usize) -> Vec<u8> {
    use rand_core::RngCore;
    let mut buf = vec![0u8; n];
    rand_core::OsRng.fill_bytes(&mut buf);
    buf
}

/// 口令 → 32 字节密钥（Argon2id，原始输出）。
pub fn derive_key(password: &str, salt: &[u8], params: KdfParams) -> Result<Zeroizing<Vec<u8>>> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let p = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(KEY_LEN))
        .map_err(|e| CercisError::Invalid(format!("Argon2 参数不合法: {e}")))?;
    let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);
    let mut out = Zeroizing::new(vec![0u8; KEY_LEN]);
    a2.hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| CercisError::Invalid(format!("口令派生失败: {e}")))?;
    Ok(out)
}

fn aead_key(key: &[u8]) -> Result<chacha20poly1305::Key> {
    if key.len() != KEY_LEN {
        return Err(CercisError::Corrupt(format!(
            "密钥长度应为 {KEY_LEN} 字节，实际 {}",
            key.len()
        )));
    }
    Ok(*chacha20poly1305::Key::from_slice(key))
}

/// 加密（AEAD）：返回 `密文 || 认证标签`。
pub fn seal(key: &[u8], nonce: &[u8], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    let cipher = chacha20poly1305::ChaCha20Poly1305::new(&aead_key(key)?);
    cipher
        .encrypt(
            chacha20poly1305::Nonce::from_slice(nonce),
            Payload { msg: plaintext, aad },
        )
        .map_err(|_| CercisError::Corrupt("加密失败（AEAD）".into()))
}

/// 解密（AEAD）；密文或 AAD 被改过都会失败。
pub fn open(key: &[u8], nonce: &[u8], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    let cipher = chacha20poly1305::ChaCha20Poly1305::new(&aead_key(key)?);
    cipher
        .decrypt(
            chacha20poly1305::Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CercisError::Corrupt("解密失败：密钥不对，或内容被改动过".into()))
}

/// 文件加密用的确定性 nonce（见模块注释）。
pub fn content_nonce(key: &[u8], plaintext: &[u8]) -> [u8; NONCE_LEN] {
    let mut h = Sha256::new();
    h.update(b"cercis-nonce/1");
    h.update(key);
    h.update(plaintext);
    let d = h.finalize();
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&d[..NONCE_LEN]);
    nonce
}

/// 内容指纹（十六进制小写 SHA-256）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// 一对 X25519 密钥。
pub struct KeyPair {
    pub secret: Zeroizing<[u8; 32]>,
    pub public: [u8; 32],
}

/// 生成一对新的 X25519 密钥。
pub fn keypair() -> KeyPair {
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&random_bytes(32));
    let secret = x25519_dalek::StaticSecret::from(seed);
    seed.zeroize();
    let public = x25519_dalek::PublicKey::from(&secret);
    KeyPair {
        secret: Zeroizing::new(secret.to_bytes()),
        public: public.to_bytes(),
    }
}

/// X25519 密钥协商。
pub fn dh(secret: &[u8; 32], peer_public: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let sk = x25519_dalek::StaticSecret::from(*secret);
    let pk = x25519_dalek::PublicKey::from(*peer_public);
    Zeroizing::new(sk.diffie_hellman(&pk).to_bytes())
}

/// HKDF-SHA256 派生。
pub fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8], out_len: usize) -> Result<Zeroizing<Vec<u8>>> {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut okm = Zeroizing::new(vec![0u8; out_len]);
    hk.expand(info, &mut okm)
        .map_err(|e| CercisError::Invalid(format!("HKDF 派生失败: {e}")))?;
    Ok(okm)
}

/// 被公钥封装过的一份内容密钥（存进 `zones.yaml`，公开也无所谓）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WrappedKey {
    /// 一次性公钥（十六进制）
    pub ephemeral_public: String,
    /// AEAD nonce（十六进制）
    pub nonce: String,
    /// 封装后的内容密钥（十六进制）
    pub ciphertext: String,
}

/// 封装用的域分隔串：把"给谁、给哪个区域、哪一代密钥"绑进加密上下文。
pub fn wrap_info(zone: &str, member: &str, generation: u32) -> String {
    format!("cercis-zone-wrap/1|{zone}|{member}|{generation}")
}

/// 用接收方公钥封装内容密钥（发送方临时生成一次性密钥对）。
pub fn wrap_content_key(
    content_key: &[u8],
    recipient_public: &[u8; 32],
    info: &str,
) -> Result<WrappedKey> {
    let ephemeral = keypair();
    let shared = dh(&ephemeral.secret, recipient_public);
    let nonce = random_bytes(NONCE_LEN);
    let wrap_key = hkdf(
        &shared[..],
        &ephemeral.public,
        format!("{info}|wrap").as_bytes(),
        KEY_LEN,
    )?;
    let aad = format!("{info}|{}", hex::encode(ephemeral.public));
    let ct = seal(&wrap_key, &nonce, aad.as_bytes(), content_key)?;
    Ok(WrappedKey {
        ephemeral_public: hex::encode(ephemeral.public),
        nonce: hex::encode(nonce),
        ciphertext: hex::encode(ct),
    })
}

/// 用接收方私钥解封内容密钥。
pub fn unwrap_content_key(
    wrapped: &WrappedKey,
    recipient_secret: &[u8; 32],
    info: &str,
) -> Result<Zeroizing<Vec<u8>>> {
    let eph = hex::decode(&wrapped.ephemeral_public)
        .map_err(|e| CercisError::Corrupt(format!("一次性公钥不是十六进制: {e}")))?;
    if eph.len() != 32 {
        return Err(CercisError::Corrupt("一次性公钥长度不对".into()));
    }
    let mut eph_arr = [0u8; 32];
    eph_arr.copy_from_slice(&eph);
    let nonce = hex::decode(&wrapped.nonce)
        .map_err(|e| CercisError::Corrupt(format!("nonce 不是十六进制: {e}")))?;
    let ct = hex::decode(&wrapped.ciphertext)
        .map_err(|e| CercisError::Corrupt(format!("密文不是十六进制: {e}")))?;

    let shared = dh(recipient_secret, &eph_arr);
    let wrap_key = hkdf(
        &shared[..],
        &eph_arr,
        format!("{info}|wrap").as_bytes(),
        KEY_LEN,
    )?;
    let aad = format!("{info}|{}", wrapped.ephemeral_public);
    open(&wrap_key, &nonce, aad.as_bytes(), &ct)
        .map(Zeroizing::new)
        .map_err(|_| {
            CercisError::Invalid("这份密钥不是发给你的（或密钥文件已损坏）".to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHT: KdfParams = KdfParams {
        m_cost: 8 * 1024,
        t_cost: 1,
        p_cost: 1,
    };

    #[test]
    fn aead_roundtrip_and_tamper_detection() {
        let key = random_bytes(KEY_LEN);
        let nonce = random_bytes(NONCE_LEN);
        let ct = seal(&key, &nonce, b"aad", "机密内容".as_bytes()).unwrap();
        assert_ne!(ct, "机密内容".as_bytes().to_vec());
        let pt = open(&key, &nonce, b"aad", &ct).unwrap();
        assert_eq!(String::from_utf8(pt).unwrap(), "机密内容");
        // 改一个字节 → 解不开
        let mut bad = ct.clone();
        bad[0] ^= 0x01;
        assert!(open(&key, &nonce, b"aad", &bad).is_err());
        // 上下文（AAD）不对 → 解不开
        assert!(open(&key, &nonce, b"other", &ct).is_err());
    }

    #[test]
    fn deterministic_nonce_is_stable_and_content_bound() {
        let key = random_bytes(KEY_LEN);
        let n1 = content_nonce(&key, b"same");
        let n2 = content_nonce(&key, b"same");
        let n3 = content_nonce(&key, b"other");
        assert_eq!(n1, n2);
        assert_ne!(n1, n3);
        let c1 = seal(&key, &n1, b"x", b"same").unwrap();
        let c2 = seal(&key, &n2, b"x", b"same").unwrap();
        assert_eq!(c1, c2, "同一明文 + 同一密钥 → 同一密文（可对账）");
    }

    #[test]
    fn password_derivation_is_stable_and_salt_dependent() {
        let salt = random_bytes(SALT_LEN);
        let k1 = derive_key("correct horse", &salt, LIGHT).unwrap();
        let k2 = derive_key("correct horse", &salt, LIGHT).unwrap();
        let k3 = derive_key("Correct horse", &salt, LIGHT).unwrap();
        let other_salt = random_bytes(SALT_LEN);
        let k4 = derive_key("correct horse", &other_salt, LIGHT).unwrap();
        assert_eq!(k1[..], k2[..]);
        assert_ne!(k1[..], k3[..]);
        assert_ne!(k1[..], k4[..]);
    }

    #[test]
    fn wrap_and_unwrap_only_for_the_right_recipient() {
        let alice = keypair();
        let bob = keypair();
        let content = random_bytes(KEY_LEN);
        let info = wrap_info("patent", "alice", 1);
        let wrapped = wrap_content_key(&content, &alice.public, &info).unwrap();
        let back = unwrap_content_key(&wrapped, &alice.secret, &info).unwrap();
        assert_eq!(back[..], content[..]);
        // 换个人解不开
        assert!(unwrap_content_key(&wrapped, &bob.secret, &info).is_err());
        // 换个上下文（另一代密钥）也解不开
        let other_info = wrap_info("patent", "alice", 2);
        assert!(unwrap_content_key(&wrapped, &alice.secret, &other_info).is_err());
    }

    #[test]
    fn fingerprint_is_short_and_content_bound() {
        let a = random_bytes(KEY_LEN);
        let b = random_bytes(KEY_LEN);
        assert_eq!(fingerprint(&a).len(), 8);
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }
}
