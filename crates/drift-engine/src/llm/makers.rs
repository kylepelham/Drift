//! Model makers' own APIs, each served by a shared adapter in its dialect: Meta, Moonshot, DeepSeek, MiniMax,
//! Alibaba, Z.AI.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Provider, anthropic, compat, openai};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Maker {
    /// Responses, over SSE only.
    Meta,
    /// Chat Completions with Kimi's `thinking` switch.
    Moonshot,
    DeepSeek,
    /// Anthropic Messages.
    MiniMax,
    /// Chat Completions on DashScope's compatible mode.
    Alibaba,
    /// Chat Completions, Z.ai's own and Zhipu's in China.
    Zai,
}

struct Route {
    id: &'static str,
    maker: Maker,
    /// For when the catalog names none.
    endpoint: &'static str,
    plan: bool,
    china: bool,
}

const fn route(id: &'static str, maker: Maker, endpoint: &'static str, plan: bool, china: bool) -> Route {
    Route {
        id,
        maker,
        endpoint,
        plan,
        china,
    }
}

/// Every route models.dev lists for them, regions and coding plans.
const ROUTES: &[Route] = &[
    route("meta", Maker::Meta, "https://api.meta.ai/v1", false, false),
    route(
        "moonshotai",
        Maker::Moonshot,
        "https://api.moonshot.ai/v1",
        false,
        false,
    ),
    route(
        "moonshotai-cn",
        Maker::Moonshot,
        "https://api.moonshot.cn/v1",
        false,
        true,
    ),
    route(
        "kimi-code-plan-global",
        Maker::Moonshot,
        "https://api.kimi.ai/coding/v1",
        true,
        false,
    ),
    route(
        "kimi-code-plan-cn",
        Maker::Moonshot,
        "https://api.kimi.com/coding/v1",
        true,
        true,
    ),
    route("deepseek", Maker::DeepSeek, "https://api.deepseek.com", false, false),
    route(
        "minimax",
        Maker::MiniMax,
        "https://api.minimax.io/anthropic/v1",
        false,
        false,
    ),
    route(
        "minimax-coding-plan",
        Maker::MiniMax,
        "https://api.minimax.io/anthropic/v1",
        true,
        false,
    ),
    route(
        "minimax-cn",
        Maker::MiniMax,
        "https://api.minimax.cn/anthropic/v1",
        false,
        true,
    ),
    route(
        "minimax-cn-coding-plan",
        Maker::MiniMax,
        "https://api.minimax.cn/anthropic/v1",
        true,
        true,
    ),
    route(
        "alibaba",
        Maker::Alibaba,
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        false,
        false,
    ),
    route(
        "alibaba-cn",
        Maker::Alibaba,
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        false,
        true,
    ),
    route(
        "alibaba-coding-plan",
        Maker::Alibaba,
        "https://coding-intl.dashscope.aliyuncs.com/v1",
        true,
        false,
    ),
    route(
        "alibaba-coding-plan-cn",
        Maker::Alibaba,
        "https://coding.dashscope.aliyuncs.com/v1",
        true,
        true,
    ),
    route("zai", Maker::Zai, "https://api.z.ai/api/paas/v4", false, false),
    route(
        "zai-coding-plan",
        Maker::Zai,
        "https://api.z.ai/api/coding/paas/v4",
        true,
        false,
    ),
    route(
        "zhipuai",
        Maker::Zai,
        "https://open.bigmodel.cn/api/paas/v4",
        false,
        true,
    ),
    route(
        "zhipuai-coding-plan",
        Maker::Zai,
        "https://open.bigmodel.cn/api/coding/paas/v4",
        true,
        true,
    ),
];

/// Where a route sits among its maker's others, so settings can show them as one provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ProviderFamily {
    /// The maker's main route, which names the family.
    pub id: String,
    pub name: String,
    /// A coding plan's endpoint rather than pay-as-you-go.
    pub plan: bool,
    pub china: bool,
}

fn find(provider: &str) -> Option<&'static Route> {
    ROUTES.iter().find(|route| route.id == provider)
}

pub(super) fn endpoint(provider: &str) -> Option<(Maker, &'static str)> {
    find(provider).map(|route| (route.maker, route.endpoint))
}

pub fn maker(provider: &str) -> Option<Maker> {
    find(provider).map(|route| route.maker)
}

/// The family a route belongs to; none for a maker with one route.
pub fn family(provider: &str) -> Option<ProviderFamily> {
    let route = find(provider)?;
    let mut siblings = ROUTES.iter().filter(|other| other.maker == route.maker);
    let main = siblings.next()?;
    siblings.next()?;

    Some(ProviderFamily {
        id: main.id.into(),
        name: route.maker.name().into(),
        plan: route.plan,
        china: route.china,
    })
}

impl Maker {
    pub(super) fn provider(self, base: &str) -> Provider {
        match self {
            Self::Meta => Provider::OpenAi(openai::OpenAi::sse(base)),
            Self::Moonshot => Provider::Compat(compat::Compat::moonshot(base)),
            Self::DeepSeek => Provider::Compat(compat::Compat::deepseek(base)),
            // models.dev names the Messages root with its version; the adapter adds `/v1/messages` itself.
            Self::MiniMax => Provider::Anthropic(anthropic::Anthropic::new(
                base.trim_end_matches('/').trim_end_matches("/v1"),
            )),
            Self::Alibaba => Provider::Compat(compat::Compat::alibaba(base)),
            Self::Zai => Provider::Compat(compat::Compat::zai(base)),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Meta => "Meta",
            Self::Moonshot => "Moonshot AI",
            Self::DeepSeek => "DeepSeek",
            Self::MiniMax => "MiniMax",
            Self::Alibaba => "Alibaba",
            Self::Zai => "Z.AI",
        }
    }

    /// Its models turn thinking off and on (`none`, `thinking`) where models.dev offers them a toggle.
    pub fn switches_thinking(self) -> bool {
        !matches!(self, Self::Meta | Self::Zai)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_makers_route_gets_the_adapter_for_its_wire() {
        let routed = |id: &str| super::super::provider_for(id, None).unwrap();

        assert!(matches!(routed("meta"), Provider::OpenAi(_)));
        assert!(matches!(routed("kimi-code-plan-cn"), Provider::Compat(_)));
        assert!(matches!(routed("deepseek"), Provider::Compat(_)));
        assert!(matches!(routed("alibaba-coding-plan"), Provider::Compat(_)));
        assert!(matches!(routed("zhipuai-coding-plan"), Provider::Compat(_)));
        let Provider::Anthropic(minimax) =
            super::super::provider_for("minimax-cn", Some("https://api.minimax.cn/anthropic/v1/")).unwrap()
        else {
            panic!("MiniMax speaks Messages");
        };
        assert_eq!(minimax.base_url, "https://api.minimax.cn/anthropic");
        assert!(ROUTES.iter().all(|route| super::super::catalog::listed(route.id)));
    }

    #[test]
    fn a_makers_routes_form_one_family_named_by_its_main_route() {
        assert_eq!(
            family("zhipuai-coding-plan"),
            Some(ProviderFamily {
                id: "zai".into(),
                name: "Z.AI".into(),
                plan: true,
                china: true,
            })
        );
        assert_eq!(
            family("kimi-code-plan-global").map(|family| family.id),
            Some("moonshotai".into())
        );
        assert_eq!(family("deepseek"), None, "one route is no family");
        assert_eq!(family("openai"), None);
    }
}
