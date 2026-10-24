//! Model makers' own APIs, each served by a shared adapter in its dialect: Meta, Moonshot, DeepSeek, MiniMax, Alibaba.

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
}

/// Every route models.dev lists for them (regions and coding plans), with its endpoint for when the catalog names none.
const ROUTES: [(&str, Maker, &str); 14] = [
    ("meta", Maker::Meta, "https://api.meta.ai/v1"),
    ("moonshotai", Maker::Moonshot, "https://api.moonshot.ai/v1"),
    ("moonshotai-cn", Maker::Moonshot, "https://api.moonshot.cn/v1"),
    (
        "kimi-code-plan-global",
        Maker::Moonshot,
        "https://api.kimi.ai/coding/v1",
    ),
    ("kimi-code-plan-cn", Maker::Moonshot, "https://api.kimi.com/coding/v1"),
    ("deepseek", Maker::DeepSeek, "https://api.deepseek.com"),
    ("minimax", Maker::MiniMax, "https://api.minimax.io/anthropic/v1"),
    (
        "minimax-coding-plan",
        Maker::MiniMax,
        "https://api.minimax.io/anthropic/v1",
    ),
    ("minimax-cn", Maker::MiniMax, "https://api.minimax.cn/anthropic/v1"),
    (
        "minimax-cn-coding-plan",
        Maker::MiniMax,
        "https://api.minimax.cn/anthropic/v1",
    ),
    (
        "alibaba",
        Maker::Alibaba,
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
    ),
    (
        "alibaba-cn",
        Maker::Alibaba,
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
    ),
    (
        "alibaba-coding-plan",
        Maker::Alibaba,
        "https://coding-intl.dashscope.aliyuncs.com/v1",
    ),
    (
        "alibaba-coding-plan-cn",
        Maker::Alibaba,
        "https://coding.dashscope.aliyuncs.com/v1",
    ),
];

pub(super) fn route(provider: &str) -> Option<(Maker, &'static str)> {
    ROUTES
        .iter()
        .find(|(id, ..)| *id == provider)
        .map(|(_, maker, endpoint)| (*maker, *endpoint))
}

pub fn maker(provider: &str) -> Option<Maker> {
    route(provider).map(|(maker, _)| maker)
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
        }
    }

    /// Its models turn thinking off and on (`none`, `thinking`) where models.dev offers them a toggle.
    pub fn switches_thinking(self) -> bool {
        self != Self::Meta
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
        let Provider::Anthropic(minimax) =
            super::super::provider_for("minimax-cn", Some("https://api.minimax.cn/anthropic/v1/")).unwrap()
        else {
            panic!("MiniMax speaks Messages");
        };
        assert_eq!(minimax.base_url, "https://api.minimax.cn/anthropic");
        assert!(ROUTES.iter().all(|(id, ..)| super::super::catalog::listed(id)));
    }
}
