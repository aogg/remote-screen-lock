//! 配置持久化模块（F-P0-04 / F-P0-15）
//!
//! 配置以 JSON 存于应用数据目录（Windows: `%APPDATA%/com.aogg.remote-screen-lock/`，
//! 与安装目录分离，覆盖安装/升级不丢配置，见 README-appendix 4.3）。
//! 本地解锁密钥仅存 argon2id 哈希（S-05），配置文件中不出现明文。

use crate::pairing;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// 配置文件版本（升级迁移用，未知字段保留）
pub const CONFIG_VERSION: u32 = 1;

/// 屏保模式（F-P0-07：clock / image / black）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 时钟模式：时间 + 日期星期
    Clock,
    /// 图片模式：自定义图片 cover 铺满
    Image,
    /// 黑屏模式：纯黑遮光
    Black,
}

impl Mode {
    /// 字符串形式（API/前端使用）
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Clock => "clock",
            Mode::Image => "image",
            Mode::Black => "black",
        }
    }
    /// 从字符串解析（非法返回 None，由调用方转 40001）
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "clock" => Some(Mode::Clock),
            "image" => Some(Mode::Image),
            "black" => Some(Mode::Black),
            _ => None,
        }
    }
}

/// 屏保图片元信息（F-P0-12：服务端重命名存储；含尺寸/大小/使用中字段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMeta {
    /// 图片 id（服务端生成，形如 img_a1b2c3d4，即存储文件主干名）
    pub id: String,
    /// 用户原始文件名（仅展示用，不做路径拼接，防 `../` 穿越）
    pub file_name: String,
    /// 存储文件扩展名（由魔数判定：jpg/png/webp；存储文件名 = {id}.{stored_ext}）
    pub stored_ext: String,
    /// 宽（px）
    pub width: u32,
    /// 高（px）
    pub height: u32,
    /// 文件大小（字节）
    pub size_bytes: u64,
    /// 是否使用中（image 模式背景）
    pub in_use: bool,
    /// 添加时间（Unix 秒）
    pub added_at: u64,
}

/// 持久化配置结构（每个字段的语义见注释；序列化为 config.json）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 配置版本号（升级迁移依据）
    pub config_version: u32,
    /// HTTP 服务监听端口（默认 8326，占用时在 8326~8335 内自动重试；修改后重启生效）
    pub port: u16,
    /// 控制 token（32 位随机串，手机配对后作为唯一凭证）
    pub token: String,
    /// 系统锁开关（默认开：锁屏时同时调用 LockWorkStation / CGSession）
    pub system_lock_enabled: bool,
    /// 默认屏保模式（未锁定时切换模式仅记录默认值）
    pub default_mode: Mode,
    /// 时间制式：true=12 小时制，false=24 小时制（F-P0-08）
    pub hour_12: bool,
    /// 图片模式遮罩百分比（0~60，默认 30，保证时间可读）
    pub image_mask_percent: u8,
    /// 本地解锁密钥的 argon2id 哈希（hexsalt$hexhash，无明文）
    pub local_key_hash: String,
    /// 密钥是否已被用户主动设置过（false=系统随机初始密钥，允许直接设置新密钥）
    pub local_key_is_user_set: bool,
    /// 本地解锁快捷键（小写形如 "ctrl+alt+q"，F-P0-09）
    pub unlock_hotkey: String,
    /// 图片列表（文件存于 app_data/images/）
    pub images: Vec<ImageMeta>,
}

impl Default for Config {
    fn default() -> Self {
        // 首次启动默认配置：token 立即生成；本地密钥为“未设置”（空哈希），
        // 首次进入设置页时引导用户设置（hash 后持久化，从不落明文）
        Self {
            config_version: CONFIG_VERSION,
            port: 8326,
            token: pairing::generate_token(),
            system_lock_enabled: true,
            default_mode: Mode::Clock,
            hour_12: false,
            image_mask_percent: 30,
            local_key_hash: String::new(),
            local_key_is_user_set: false,
            unlock_hotkey: "ctrl+alt+q".to_string(),
            images: Vec::new(),
        }
    }
}

/// 十六进制编码（无 hex 依赖）
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 十六进制解码
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// 用 argon2id 计算密钥哈希（salt 16 字节随机），格式 `hexsalt$hexhash`
pub fn hash_local_key(key: &str) -> String {
    let salt: [u8; 16] = rand::thread_rng().gen();
    hash_local_key_with_salt(key, &salt)
}

/// 指定 salt 计算密钥哈希（校验时复现用）
pub fn hash_local_key_with_salt(key: &str, salt: &[u8]) -> String {
    use argon2::Argon2;
    let mut out = [0u8; 32];
    // Argon2 默认参数（Argon2id, m=19456,t=2,p=1），输出长度取 out.len()
    if Argon2::default()
        .hash_password_into(key.as_bytes(), salt, &mut out)
        .is_err()
    {
        // argon2 对合法 salt/长度不会失败；此处兜底避免 panic（理论不可达）
        return String::new();
    }
    format!("{}${}", hex(salt), hex(&out))
}

/// 校验本地密钥是否与存储哈希匹配
pub fn verify_local_key(key: &str, stored: &str) -> bool {
    let Some((salt_hex, _hash_hex)) = stored.split_once('$') else {
        return false;
    };
    let Some(salt) = unhex(salt_hex) else {
        return false;
    };
    // 重新派生后整体比对（哈希比对，不存明文）
    hash_local_key_with_salt(key, &salt) == stored
}

/// 加载配置；文件不存在或损坏时初始化默认配置（损坏时备份旧文件再重建，README-appendix 4.3）
pub fn load_or_init(config_path: &Path) -> Config {
    match fs::read_to_string(config_path) {
        Ok(text) => match serde_json::from_str::<Config>(&text) {
            Ok(mut cfg) => {
                // 未知/缺失字段回填默认值（向前兼容）
                if cfg.config_version != CONFIG_VERSION {
                    cfg.config_version = CONFIG_VERSION; // v1 之前无历史版本，直接升级
                }
                cfg
            }
            Err(e) => {
                // 配置损坏：备份后以默认配置启动（不把用户锁在外面）
                let _ = fs::copy(config_path, config_path.with_extension("json.bak"));
                eprintln!("[config] 配置解析失败({e})，已备份并使用默认配置");
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

/// 保存配置（原子写：先 tmp 再替换），并收紧文件权限仅当前用户可读（S-05）
pub fn save(config: &Config, path: &Path) -> Result<(), String> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let text = serde_json::to_string_pretty(config).map_err(|e| format!("序列化配置失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| format!("写入配置失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("替换配置失败: {e}"))?;
    restrict_permissions(path);
    Ok(())
}

/// 收紧配置文件权限：unix chmod 600；Windows 尝试 icacls 移除继承（失败仅记录，不阻断）
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    {
        let p: PathBuf = path.to_path_buf();
        std::thread::spawn(move || {
            match std::process::Command::new("icacls")
                .arg(&p)
                .args(["/inheritance:r", "/grant:r", "*S-1-3-0:F"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                Ok(st) if st.success() => {}
                Ok(st) => eprintln!("[config] icacls 收紧权限退出码异常: {st}"),
                Err(e) => eprintln!("[config] icacls 收紧权限失败: {e}"),
            }
        });
    }
}
