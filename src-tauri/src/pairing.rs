//! 配对与凭证模块（F-P0-04 / F-P0-05 / S-01~S-03）
//!
//! - token：32 位随机串（CSPRNG），重启保持不变（持久化于 config）
//! - 配对码：8 位（去掉易混淆的 0/1/I/O），10 分钟有效、一次性
//! - 二维码：qrcode crate 纯 Rust 生成 SVG（内容 `http://<局域网IP>:<端口>/pair#t=<token>`）

use qrcode::render::svg;
use qrcode::{EcLevel, QrCode};
use rand::Rng;

/// 配对码字母表：30 个无歧义字符（无 0/1/I/O）
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// 配对码有效期（秒）
pub const PAIRING_TTL_SECS: u64 = 600;

/// 生成 32 位随机 token（字母数字，CSPRNG；S-01）
pub fn generate_token() -> String {
    use rand::distributions::Alphanumeric;
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

/// 生成 8 位配对码（大写字母+数字，无歧义字符）
pub fn generate_pairing_code() -> String {
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

/// 配对码展示格式：4K7 QM2 ZP（3-3-2 分组）
pub fn format_pairing_code(code: &str) -> String {
    let c: Vec<char> = code.chars().collect();
    format!("{} {} {}", c[0..3].iter().collect::<String>(), c[3..6].iter().collect::<String>(), c[6..8].iter().collect::<String>())
}

/// 归一化用户输入的配对码：去空格/横线、转大写（输入 "4k7-qm2zp" 也可通过）
pub fn normalize_pairing_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// 生成随机图片 id：img_ + 8 位 hex
pub fn generate_image_id() -> String {
    let bytes: [u8; 4] = rand::thread_rng().gen();
    format!("img_{}", crate::config::hex(&bytes))
}

/// 生成配对 URL 的二维码 SVG 字符串（240x240，黑/白）
pub fn qr_svg(url: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(url.as_bytes(), EcLevel::L)
        .map_err(|e| format!("生成二维码失败: {e}"))?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .build())
}

/// token 脱敏展示：仅前 4 位（S-01：日志/界面最多出现前 4 位）
pub fn mask_token(token: &str) -> String {
    let head: String = token.chars().take(4).collect();
    format!("{head}****")
}
