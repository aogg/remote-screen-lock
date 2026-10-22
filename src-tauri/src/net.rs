//! 局域网地址枚举（配对地址展示 / 二维码内容）

use std::net::IpAddr;

/// 枚举本机局域网 IPv4 地址（排除回环/链路本地）。
/// 返回顺序：私有网段（10./172.16-31./192.168.）优先；无任何地址时回退 127.0.0.1。
pub fn lan_ipv4_addrs() -> Vec<IpAddr> {
    let mut candidates: Vec<IpAddr> = Vec::new();
    // if_addrs 枚举失败（极端环境）时仅回退回环地址，保证 UI 仍可用
    match if_addrs::get_if_addrs() {
        Ok(ifaces) => {
            for iface in ifaces {
                let ip = iface.ip();
                if !ip.is_loopback() {
                    candidates.push(ip);
                }
            }
        }
        Err(e) => eprintln!("[net] 枚举网卡失败: {e}"),
    }
    // 仅保留 IPv4；IPv6 二维码可读性差且一期面向局域网
    candidates.retain(|ip| matches!(ip, IpAddr::V4(_)));
    // 链路本地（169.254.x.x）排到最后，私有网段靠前
    candidates.sort_by_key(|ip| {
        let private = match ip {
            IpAddr::V4(v4) => v4.is_private(),
            _ => false,
        };
        !private // false(私有) 排前
    });
    if candidates.is_empty() {
        candidates.push(IpAddr::from([127, 0, 0, 1]));
    }
    candidates
}

/// 生成全部局域网访问地址：http://<ip>:<port>
pub fn lan_base_urls(port: u16) -> Vec<String> {
    lan_ipv4_addrs()
        .into_iter()
        .map(|ip| format!("http://{ip}:{port}"))
        .collect()
}
