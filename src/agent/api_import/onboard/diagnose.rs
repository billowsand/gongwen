//! 试调结果分类：失败卡在哪一步、谁能修。程序先判断，模型只处理要对照资料改配置的那几类。

use crate::agent::api::{ApiEndpoint, Trial, looks_like_html};

/// 一次试调的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// 调通了。
    Ok,
    /// 没发出去：配置不全（缺密钥、缺样例值、地址不完整……）。
    Blocked(String),
    /// 发不出去或没回音：地址不对、没接内网、超时。
    Unreachable(String),
    /// 鉴权没过。`has_secret`：接口带了密钥（Key 可能不对）；没带（不知道怎么带）。
    Auth { has_secret: bool },
    /// 404：路径不对。
    NotFound,
    /// 405：方法不对。
    MethodNotAllowed,
    /// 400 / 415 / 422：参数名、类型或请求体格式不对。
    BadRequest,
    /// 5xx：多半也是请求格式不对，少数是对方服务坏了。
    Server(u16),
    /// HTTP 成功，但返回体说查询失败。
    Business,
    /// 返回了，但和返回映射对不上（列表位置、不是 JSON）。
    Mapping,
}

impl Verdict {
    /// 要对照资料改配置、交给模型的。
    pub(crate) fn fixable(&self) -> bool {
        matches!(
            self,
            Self::NotFound
                | Self::MethodNotAllowed
                | Self::BadRequest
                | Self::Server(_)
                | Self::Business
                | Self::Mapping
        )
    }

    /// 给人和模型看的一句话。
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Ok => "调通了".into(),
            Self::Blocked(why) => format!("还没法发请求：{why}"),
            Self::Unreachable(why) => format!("连不上：{why}"),
            Self::Auth { has_secret: true } => "鉴权没过：Key 可能不对、过期，或者带法不对".into(),
            Self::Auth { has_secret: false } => "接口要鉴权，但没配密钥".into(),
            Self::NotFound => "返回 404：地址路径不对".into(),
            Self::MethodNotAllowed => "返回 405：请求方法不对".into(),
            Self::BadRequest => "参数不对：参数名、类型或请求体格式和接口要的不一样".into(),
            Self::Server(status) => format!("返回 {status}：对方服务出错，多半是请求格式不对"),
            Self::Business => "接口说查询失败：看返回里的提示".into(),
            Self::Mapping => "拿到了返回，但和返回映射对不上".into(),
        }
    }

    /// 交给模型时附的修法提示。
    pub(crate) fn hint(&self) -> &'static str {
        match self {
            Self::NotFound => {
                "核对资料里的路径：是不是少了前缀（如 /api、网关名）、多了或少了斜杠、大小写不对。"
            }
            Self::MethodNotAllowed => "核对资料里写的请求方式；只能用 GET 或 POST。",
            Self::BadRequest => {
                "核对参数名、类型与位置：GET 参数放地址上，POST 放 JSON 请求体；看返回里的提示是哪个参数。"
            }
            Self::Server(_) => {
                "多半是请求体格式或必填参数不对；按资料核对请求体结构。资料与配置都对的话，说明对方服务有问题，调 finish 说明。"
            }
            Self::Business => {
                "看返回里的提示（msg / message）说什么：参数缺了、取值不对，还是成功判据配错了（例如成功码其实是 200 不是 0）。"
            }
            Self::Mapping => {
                "按真实返回改返回映射：列表位置写成 JSON 指针（如 /data/rows），成功判据写成功码的位置与取值。"
            }
            _ => "",
        }
    }
}

/// 给一次试调下结论。
pub(crate) fn verdict(trial: &Trial, endpoint: &ApiEndpoint) -> Verdict {
    if trial.ok() {
        return Verdict::Ok;
    }
    let error = trial.error.clone().unwrap_or_default();
    let Some(raw) = &trial.raw else {
        // 组好了请求却没拿到返回：网络问题；连请求都没组成：配置不全。
        return if trial.request.is_some() {
            Verdict::Unreachable(short(&error))
        } else {
            Verdict::Blocked(short(&error))
        };
    };
    if trial.auth_rejected() {
        return Verdict::Auth {
            has_secret: !endpoint.secret_names().is_empty(),
        };
    }
    match raw.status {
        404 => Verdict::NotFound,
        405 => Verdict::MethodNotAllowed,
        400 | 415 | 422 => Verdict::BadRequest,
        500..=599 => Verdict::Server(raw.status),
        200..=299 if error.starts_with("接口报告查询失败") => Verdict::Business,
        200..=299 => Verdict::Mapping,
        // 其他 4xx（409、429……）也当参数问题交给模型看返回。
        _ if looks_like_html(&raw.body) => Verdict::NotFound,
        _ => Verdict::BadRequest,
    }
}

fn short(text: &str) -> String {
    crate::agent::tools::short(text, 120)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::api::RawResponse;

    fn failed(status: u16, body: &str, error: &str) -> Trial {
        Trial {
            request: Some("GET http://x".into()),
            raw: Some(RawResponse {
                status,
                body: body.into(),
            }),
            error: Some(error.into()),
            ..Trial::default()
        }
    }

    #[test]
    fn failures_are_sorted_by_who_can_fix_them() {
        let endpoint = ApiEndpoint::default();
        assert_eq!(verdict(&Trial::default(), &endpoint), Verdict::Ok);
        let blocked = Trial {
            error: Some("密钥「token」还没有填".into()),
            ..Trial::default()
        };
        assert!(matches!(verdict(&blocked, &endpoint), Verdict::Blocked(_)));
        let down = Trial {
            request: Some("GET http://10.0.0.9".into()),
            error: Some("连接被拒绝".into()),
            ..Trial::default()
        };
        assert!(matches!(verdict(&down, &endpoint), Verdict::Unreachable(_)));
        assert_eq!(
            verdict(&failed(401, "", "接口返回 401"), &endpoint),
            Verdict::Auth { has_secret: false }
        );
        assert_eq!(verdict(&failed(404, "", "x"), &endpoint), Verdict::NotFound);
        assert_eq!(
            verdict(&failed(400, "{}", "x"), &endpoint),
            Verdict::BadRequest
        );
        assert_eq!(
            verdict(
                &failed(200, "{\"code\": 1}", "接口报告查询失败（/code 是 1）"),
                &endpoint
            ),
            Verdict::Business
        );
        assert_eq!(
            verdict(&failed(200, "{}", "返回里找不到列表位置 /data"), &endpoint),
            Verdict::Mapping
        );
        assert!(Verdict::Mapping.fixable() && !Verdict::Auth { has_secret: true }.fixable());
    }
}
