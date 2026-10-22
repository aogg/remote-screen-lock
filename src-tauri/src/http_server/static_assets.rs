//! 控制台静态资源内嵌（F-P0-03 / O-01：零外网依赖，无 CDN，资源全部打包进二进制）。
//! 方案取 tech-notes 3 的方案 3：include_str!/include_bytes! + 手写路由（页面少，无额外依赖）。

/// 静态资源内容 + MIME 类型
pub struct StaticAsset {
    /// 字节内容
    pub bytes: &'static [u8],
    /// Content-Type
    pub mime: &'static str,
}

/// 按请求路径取内嵌资源；仅暴露手机端页面（桌面 settings/lock 页不经 HTTP 提供）
pub fn lookup(path: &str) -> Option<StaticAsset> {
    // ////////////////  核心逻辑：路由表  start  ///////////////////////////////
    let asset = match path {
        "/" | "/console" | "/index.html" => StaticAsset {
            bytes: include_bytes!("../../../frontend/index.html"),
            mime: "text/html; charset=utf-8",
        },
        "/pair" => StaticAsset {
            bytes: include_bytes!("../../../frontend/pair.html"),
            mime: "text/html; charset=utf-8",
        },
        "/manifest.webmanifest" => StaticAsset {
            bytes: include_bytes!("../../../frontend/assets/manifest.webmanifest"),
            mime: "application/manifest+json; charset=utf-8",
        },
        "/assets/app.css" => StaticAsset {
            bytes: include_bytes!("../../../frontend/assets/app.css"),
            mime: "text/css; charset=utf-8",
        },
        "/assets/console.js" => StaticAsset {
            bytes: include_bytes!("../../../frontend/assets/console.js"),
            mime: "text/javascript; charset=utf-8",
        },
        "/assets/pair.js" => StaticAsset {
            bytes: include_bytes!("../../../frontend/assets/pair.js"),
            mime: "text/javascript; charset=utf-8",
        },
        "/assets/icons/icon-192.png" => StaticAsset {
            bytes: include_bytes!("../../../frontend/assets/icons/icon-192.png"),
            mime: "image/png",
        },
        "/assets/icons/icon-512.png" => StaticAsset {
            bytes: include_bytes!("../../../frontend/assets/icons/icon-512.png"),
            mime: "image/png",
        },
        "/favicon.ico" => StaticAsset {
            // favicon 直接复用 192 图标（png）
            bytes: include_bytes!("../../../frontend/assets/icons/icon-192.png"),
            mime: "image/png",
        },
        _ => return None,
    };
    Some(asset)
    // ////////////////  核心逻辑：路由表  end  /////////////////////////////////
}
