use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Prices per million tokens. A long prompt can cost more: `tiers` are models.dev's context tiers and
/// its `context_over_200k`, and the largest one a request's prompt passes prices the whole request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(from = "RawPrices")]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<CostTier>,
}

/// The prices for a request whose prompt is longer than `above` tokens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CostTier {
    pub above: u64,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Prices from models.dev's context tiers or from an already converted cached catalog.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawPrices {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_write: f64,
    tiers: Vec<RawTier>,
    context_over_200k: Option<RawTier>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawTier {
    above: Option<u64>,
    tier: Option<TierSize>,
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Deserialize)]
struct TierSize {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    size: u64,
}

impl Cost {
    /// The `(input, output, cache_read, cache_write)` prices for a request with a prompt of `prompt` tokens.
    pub fn at(&self, prompt: u64) -> (f64, f64, f64, f64) {
        let tier = self
            .tiers
            .iter()
            .filter(|tier| prompt > tier.above)
            .max_by_key(|tier| tier.above);
        let base = (self.input, self.output, self.cache_read, self.cache_write);

        tier.map_or(base, |tier| {
            (tier.input, tier.output, tier.cache_read, tier.cache_write)
        })
    }
}

impl From<RawPrices> for Cost {
    fn from(raw: RawPrices) -> Self {
        let (input, output, cache_read, cache_write) = (raw.input, raw.output, raw.cache_read, raw.cache_write);
        let tier = |above: u64, given: RawTier| CostTier {
            above,
            input: given.input.unwrap_or(input),
            output: given.output.unwrap_or(output),
            cache_read: given.cache_read.unwrap_or(cache_read),
            cache_write: given.cache_write.unwrap_or(cache_write),
        };

        let mut tiers = Vec::new();
        for given in raw.tiers {
            let context_size = given
                .tier
                .as_ref()
                .filter(|size| size.kind.as_deref().is_none_or(|kind| kind == "context"));
            let above = given.above.or(context_size.map(|size| size.size));
            if let Some(above) = above {
                tiers.push(tier(above, given));
            }
        }

        // models.dev may include both forms of the same 200k tier.
        if let Some(over) = raw
            .context_over_200k
            .filter(|_| !tiers.iter().any(|tier| tier.above == 200_000))
        {
            tiers.push(tier(200_000, over));
        }
        tiers.sort_by_key(|tier| tier.above);

        Self {
            input,
            output,
            cache_read,
            cache_write,
            tiers,
        }
    }
}
