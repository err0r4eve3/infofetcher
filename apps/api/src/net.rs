use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use reqwest::{Response, Url, header::HeaderMap};
use tokio::net::lookup_host;

use crate::error::{AppError, AppResult};

const MAX_REDIRECTS: usize = 5;

pub async fn validate_http_url(raw: &str, allow_private: bool) -> AppResult<Url> {
    let url = Url::parse(raw).map_err(|err| AppError::BadRequest(format!("URL 无效: {err}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::BadRequest("只允许 http/https URL".to_string()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::BadRequest("URL 不允许包含凭据".to_string()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AppError::BadRequest("URL 缺少 hostname".to_string()))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        if !allow_private {
            return Err(AppError::BadRequest(
                "禁止访问本机或私有网络地址".to_string(),
            ));
        }
        return Ok(url);
    }

    let port = url
        .port_or_known_default()
        .ok_or_else(|| AppError::BadRequest("URL 端口无效".to_string()))?;
    let addresses = lookup_host((host, port))
        .await
        .map_err(|err| AppError::Upstream(format!("无法解析 hostname: {err}")))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(AppError::Upstream("hostname 没有可用地址".to_string()));
    }
    if !allow_private
        && addresses
            .iter()
            .any(|address| ip_is_private_or_reserved(address.ip()))
    {
        return Err(AppError::BadRequest(
            "禁止访问本机、私有、链路本地或保留地址".to_string(),
        ));
    }
    Ok(url)
}

pub async fn get_with_safe_redirects(
    http: &reqwest::Client,
    raw_url: &str,
    headers: &HeaderMap,
    allow_private: bool,
) -> AppResult<Response> {
    let mut current = validate_http_url(raw_url, allow_private).await?;
    let origin_host = current.host_str().map(str::to_ascii_lowercase);

    for redirect_count in 0..=MAX_REDIRECTS {
        let mut next_headers = headers.clone();
        if current.host_str().map(str::to_ascii_lowercase) != origin_host {
            next_headers.remove(reqwest::header::COOKIE);
            next_headers.remove(reqwest::header::AUTHORIZATION);
        }
        let response = http
            .get(current.clone())
            .headers(next_headers)
            .send()
            .await?;
        if !is_followable_redirect(response.status()) {
            return Ok(response);
        }
        if redirect_count == MAX_REDIRECTS {
            return Err(AppError::Upstream("重定向次数过多".to_string()));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| AppError::Upstream("重定向响应缺少 Location".to_string()))?;
        let next = current
            .join(location)
            .map_err(|err| AppError::Upstream(format!("重定向 URL 无效: {err}")))?;
        current = validate_http_url(next.as_str(), allow_private).await?;
    }

    Err(AppError::Upstream("重定向处理失败".to_string()))
}

fn is_followable_redirect(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::MOVED_PERMANENTLY
            | reqwest::StatusCode::FOUND
            | reqwest::StatusCode::SEE_OTHER
            | reqwest::StatusCode::TEMPORARY_REDIRECT
            | reqwest::StatusCode::PERMANENT_REDIRECT
    )
}

pub async fn read_response_limited(mut response: Response, limit: usize) -> AppResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(AppError::Upstream(format!(
            "响应超过大小限制（{} bytes）",
            limit
        )));
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(AppError::Upstream(format!(
                "响应超过大小限制（{} bytes）",
                limit
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(crate) fn ip_is_private_or_reserved(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ipv4_is_private_or_reserved(ip),
        IpAddr::V6(ip) => ipv6_is_private_or_reserved(ip),
    }
}

fn ipv4_is_private_or_reserved(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip == Ipv4Addr::BROADCAST
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 198 && matches!(octets[1], 18 | 19))
        || octets[0] >= 240
}

fn ipv6_is_private_or_reserved(ip: Ipv6Addr) -> bool {
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        return ipv4_is_private_or_reserved(ipv4);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_url_credentials_before_dns_or_redirect_fetch() {
        assert!(
            validate_http_url("https://user:secret@example.invalid/", true)
                .await
                .is_err()
        );
        assert!(
            validate_http_url("https://user@example.invalid/", false)
                .await
                .is_err()
        );
    }

    #[test]
    fn rejects_private_and_reserved_addresses() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "198.18.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            let ip = address.parse().unwrap();
            assert!(ip_is_private_or_reserved(ip), "{address}");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for address in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            let ip = address.parse().unwrap();
            assert!(!ip_is_private_or_reserved(ip), "{address}");
        }
    }

    #[test]
    fn only_follows_redirects_that_require_a_location() {
        assert!(is_followable_redirect(
            reqwest::StatusCode::MOVED_PERMANENTLY
        ));
        assert!(is_followable_redirect(reqwest::StatusCode::FOUND));
        assert!(is_followable_redirect(reqwest::StatusCode::SEE_OTHER));
        assert!(is_followable_redirect(
            reqwest::StatusCode::TEMPORARY_REDIRECT
        ));
        assert!(is_followable_redirect(
            reqwest::StatusCode::PERMANENT_REDIRECT
        ));
        assert!(!is_followable_redirect(reqwest::StatusCode::NOT_MODIFIED));
    }
}
