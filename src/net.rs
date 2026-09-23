//! 访问模型服务的 HTTP 客户端。
//!
//! 起草、复核、知识库的 embedding / rerank 都从这里拿客户端，代理设置只在
//! 这一处生效。代理配置放在进程级的全局里，而不是一路传进各个调用点：请求
//! 散在后台线程里，每个入口手里只有自己那份接口配置，为了代理把 [`AppConfig`]
//! 一路穿下去不值当。
//!
//! [`AppConfig`]: crate::models::AppConfig

use crate::models::{ProxyConfig, ProxyMode};
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::{NoProxy, Proxy, Url};
use std::net::IpAddr;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

static PROXY: LazyLock<RwLock<ProxyConfig>> = LazyLock::new(Default::default);

/// 支持的代理协议。`socks5h` 让代理端解析域名，本机 DNS 不可信时用它。
const SCHEMES: [&str; 6] = ["http", "https", "socks4", "socks4a", "socks5", "socks5h"];

/// 换上新的代理配置，此后新建的客户端按它走。界面每帧调一次，没变时只读
/// 不写，不跟后台线程抢写锁。
pub fn set_proxy(config: &ProxyConfig) {
    if *PROXY.read().unwrap_or_else(|e| e.into_inner()) == *config {
        return;
    }
    *PROXY.write().unwrap_or_else(|e| e.into_inner()) = config.clone();
}

fn current_proxy() -> ProxyConfig {
    PROXY.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 为访问 `target`（接口地址）建一个客户端。
pub fn client(target: &str, timeout_seconds: u64) -> Result<Client> {
    build(&current_proxy(), target, timeout_seconds)
}

fn build(config: &ProxyConfig, target: &str, timeout_seconds: u64) -> Result<Client> {
    let builder = Client::builder().timeout(Duration::from_secs(timeout_seconds.max(5)));
    let builder = if is_loopback(target) {
        builder.no_proxy()
    } else {
        match config.mode {
            ProxyMode::Direct => builder.no_proxy(),
            // reqwest 默认就读环境变量与系统代理。
            ProxyMode::System => builder,
            ProxyMode::Custom => {
                let url = parse_proxy_url(&config.url)?;
                let proxy = Proxy::all(url)
                    .context("代理地址无效")?
                    .no_proxy(NoProxy::from_string(&config.bypass));
                builder.proxy(proxy)
            }
        }
    };
    builder.build().context("创建 HTTP 客户端失败")
}

/// 校验并规整代理地址。没写协议的按 `http://` 补上——多数代理软件界面上
/// 只显示 `127.0.0.1:7890`，照抄过来就该能用。
pub fn parse_proxy_url(raw: &str) -> Result<Url> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("已选「自定义代理」，但没有填写代理地址");
    }
    let text = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let url = Url::parse(&text).with_context(|| format!("代理地址无法识别：{raw}"))?;
    if !SCHEMES.contains(&url.scheme()) {
        bail!(
            "不支持的代理协议 {}，可用：{}",
            url.scheme(),
            SCHEMES.join(" / ")
        );
    }
    if url.host_str().is_none_or(str::is_empty) {
        bail!("代理地址缺少主机名：{raw}");
    }
    Ok(url)
}

/// 目标是不是本机。解析不了的地址当作非本机，交给后面的请求去报错。
fn is_loopback(target: &str) -> bool {
    let Ok(url) = Url::parse(target.trim()) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(url: &str) -> ProxyConfig {
        ProxyConfig {
            mode: ProxyMode::Custom,
            url: url.into(),
            bypass: String::new(),
        }
    }

    #[test]
    fn loopback_targets_are_recognised() {
        assert!(is_loopback("http://127.0.0.1:1234/v1"));
        assert!(is_loopback("http://127.8.9.10/v1"));
        assert!(is_loopback("http://localhost:11434/v1"));
        assert!(is_loopback("http://LOCALHOST./v1"));
        assert!(is_loopback("http://[::1]:1234/v1"));
        assert!(!is_loopback("https://api.deepseek.com/v1"));
        assert!(!is_loopback("http://192.168.1.10:1234/v1"));
        assert!(!is_loopback("not a url"));
    }

    #[test]
    fn proxy_url_without_scheme_defaults_to_http() {
        let url = parse_proxy_url(" 127.0.0.1:7890 ").unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:7890/");
        let url = parse_proxy_url("localhost:7890").unwrap();
        assert_eq!(url.scheme(), "http");
    }

    #[test]
    fn proxy_url_accepts_socks_with_credentials() {
        let url = parse_proxy_url("socks5h://user:pa%40ss@10.0.0.2:1080").unwrap();
        assert_eq!(url.scheme(), "socks5h");
        assert_eq!(url.username(), "user");
    }

    #[test]
    fn proxy_url_rejects_bad_input() {
        assert!(parse_proxy_url("").is_err());
        assert!(parse_proxy_url("ftp://10.0.0.2:21").is_err());
        assert!(parse_proxy_url("http://").is_err());
    }

    #[test]
    fn custom_mode_builds_for_every_scheme() {
        for scheme in SCHEMES {
            let config = custom(&format!("{scheme}://127.0.0.1:1080"));
            assert!(
                build(&config, "https://api.example.com/v1", 30).is_ok(),
                "{scheme}"
            );
        }
    }

    #[test]
    fn custom_mode_without_url_fails_instead_of_going_direct() {
        let err = build(&custom(""), "https://api.example.com/v1", 30).unwrap_err();
        assert!(format!("{err:#}").contains("没有填写代理地址"));
    }

    #[test]
    fn loopback_target_ignores_broken_custom_proxy() {
        assert!(build(&custom(""), "http://127.0.0.1:1234/v1", 30).is_ok());
    }

    /// 起一个只接一次连接的本机服务，把收到的头几个字节交回来，再答一个
    /// 固定的响应。
    fn one_shot_server(
        respond: impl FnOnce(&mut std::net::TcpStream) -> Vec<u8> + Send + 'static,
    ) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            respond(&mut stream)
        });
        (port, handle)
    }

    const OK: &[u8] = b"HTTP/1.1 200 OK
Content-Length: 11
Connection: close

{\"data\":[]}";

    #[test]
    fn http_proxy_receives_the_request() {
        use std::io::{Read, Write};
        let (port, handle) = one_shot_server(|stream| {
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).unwrap();
            stream.write_all(OK).unwrap();
            buf[..n].to_vec()
        });
        let client = build(
            &custom(&format!("127.0.0.1:{port}")),
            "http://model.example.invalid/v1",
            10,
        )
        .unwrap();
        let body = client
            .get("http://model.example.invalid/v1/models")
            .send()
            .unwrap()
            .text()
            .unwrap();
        assert_eq!(body, "{\"data\":[]}");
        let seen = String::from_utf8(handle.join().unwrap()).unwrap();
        assert!(
            seen.starts_with("GET http://model.example.invalid/v1/models HTTP/1.1"),
            "{seen}"
        );
    }

    #[test]
    fn socks5h_proxy_receives_the_hostname() {
        use std::io::{Read, Write};
        let (port, handle) = one_shot_server(|stream| {
            // 问候：VER NMETHODS METHODS…，答「无需认证」。
            let mut head = [0u8; 2];
            stream.read_exact(&mut head).unwrap();
            let mut methods = vec![0u8; head[1] as usize];
            stream.read_exact(&mut methods).unwrap();
            stream.write_all(&[5, 0]).unwrap();
            // CONNECT：VER CMD RSV ATYP=3(域名) LEN 域名 PORT。
            let mut req = [0u8; 5];
            stream.read_exact(&mut req).unwrap();
            assert_eq!(&req[..4], &[5, 1, 0, 3]);
            let mut host = vec![0u8; req[4] as usize];
            stream.read_exact(&mut host).unwrap();
            let mut target_port = [0u8; 2];
            stream.read_exact(&mut target_port).unwrap();
            stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
            // 隧道打通后就是原样的 HTTP。
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).unwrap();
            stream.write_all(OK).unwrap();
            host
        });
        let client = build(
            &custom(&format!("socks5h://127.0.0.1:{port}")),
            "http://model.example.invalid/v1",
            10,
        )
        .unwrap();
        let status = client
            .get("http://model.example.invalid/v1/models")
            .send()
            .unwrap()
            .status();
        assert!(status.is_success());
        assert_eq!(handle.join().unwrap(), b"model.example.invalid");
    }

    #[test]
    fn bypass_list_goes_direct() {
        use std::io::{Read, Write};
        // 目标本身就是这个服务：绕过代理才连得上，走了代理（一个没人听的端口）就会失败。
        let (port, handle) = one_shot_server(|stream| {
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).unwrap();
            stream.write_all(OK).unwrap();
            buf[..n].to_vec()
        });
        let config = ProxyConfig {
            mode: ProxyMode::Custom,
            url: "http://192.0.2.1:9".into(),
            bypass: "127.0.0.0/8".into(),
        };
        // 用网段 IP 而不是 127.0.0.1 的字面判断：走的是 bypass 规则，不是本机直连。
        let target = format!("http://127.0.0.1:{port}/v1");
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .proxy(
                Proxy::all(parse_proxy_url(&config.url).unwrap())
                    .unwrap()
                    .no_proxy(NoProxy::from_string(&config.bypass)),
            )
            .build()
            .unwrap();
        client.get(format!("{target}/models")).send().unwrap();
        let seen = String::from_utf8(handle.join().unwrap()).unwrap();
        assert!(seen.starts_with("GET /v1/models HTTP/1.1"), "{seen}");
    }
}
