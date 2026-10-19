//! One way to ask for a size, read by each provider in its own terms: OpenAI takes pixels, Google and
//! xAI an aspect ratio from a fixed list and a resolution tier.

/// `auto`, a ratio such as `16:9`, or `WIDTHxHEIGHT` in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Auto,
    /// Width over height.
    Ratio(f64),
    Pixels(u32, u32),
}

/// OpenAI's standard sizes, for a call that names only a ratio.
const OPENAI_SIZES: [&str; 3] = ["1024x1024", "1536x1024", "1024x1536"];
pub(super) const XAI_RATIOS: [&str; 15] = [
    "1:1", "3:4", "4:3", "9:16", "16:9", "2:3", "3:2", "9:19.5", "19.5:9", "9:20", "20:9", "1:2", "2:1", "21:9", "5:2",
];
pub(super) const XAI_TIERS: [(u32, &str); 3] = [(1024, "1k"), (1536, "1.5k"), (u32::MAX, "2k")];
pub(super) const GOOGLE_RATIOS: [&str; 14] = [
    "1:1", "1:4", "4:1", "1:8", "8:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9",
];
pub(super) const GOOGLE_TIERS: [(u32, &str); 4] = [(512, "512"), (1024, "1K"), (2048, "2K"), (u32::MAX, "4K")];

/// `a:b` or `axb` as two positive numbers.
fn pair(text: &str, separator: char) -> Option<(f64, f64)> {
    let (left, right) = text.split_once(separator)?;
    let left: f64 = left.trim().parse().ok()?;
    let right: f64 = right.trim().parse().ok()?;

    (left > 0.0 && right > 0.0 && left.is_finite() && right.is_finite()).then_some((left, right))
}

/// Width over height of `1:1` or `1024x1024`.
fn shape(option: &str) -> f64 {
    let (width, height) = pair(option, ':').or_else(|| pair(option, 'x')).unwrap_or((1.0, 1.0));

    width / height
}

impl Size {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim().to_ascii_lowercase();
        if text.is_empty() || text == "auto" {
            return Ok(Self::Auto);
        }
        if let Some((width, height)) = pair(&text, ':') {
            return Ok(Self::Ratio(width / height));
        }
        let pixels = pair(&text, 'x').filter(|(width, height)| width.fract() == 0.0 && height.fract() == 0.0);
        if let Some((width, height)) = pixels {
            return Ok(Self::Pixels(width as u32, height as u32));
        }

        Err(format!(
            "`{text}` is not a size: use auto, a ratio such as 16:9, or WIDTHxHEIGHT such as 1536x1024"
        ))
    }

    fn ratio(self) -> Option<f64> {
        match self {
            Self::Auto => None,
            Self::Ratio(ratio) => Some(ratio),
            Self::Pixels(width, height) => Some(f64::from(width) / f64::from(height)),
        }
    }

    /// OpenAI's `size`: pixels as given, a ratio as the nearest standard size.
    pub(super) fn openai(self) -> Option<String> {
        match self {
            Self::Pixels(width, height) => Some(format!("{width}x{height}")),
            _ => self.ratio_among(&OPENAI_SIZES).map(str::to_string),
        }
    }

    /// The option in `options` nearest this size's shape; none for `auto`.
    pub(super) fn ratio_among(self, options: &[&'static str]) -> Option<&'static str> {
        let ratio = self.ratio()?.ln();
        let distance = |option: &&str| (shape(option).ln() - ratio).abs();

        options
            .iter()
            .copied()
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
    }

    /// The first tier as large as the longest side asked for; none unless pixels were named.
    pub(super) fn tier(self, tiers: &[(u32, &'static str)]) -> Option<&'static str> {
        let Self::Pixels(width, height) = self else {
            return None;
        };
        let side = width.max(height);

        tiers.iter().find(|(limit, _)| side <= *limit).map(|(_, tier)| *tier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_read_as_ratios_or_pixels_and_mapped_per_provider() {
        assert_eq!(Size::parse(" Auto "), Ok(Size::Auto));
        assert_eq!(Size::parse("16:9"), Ok(Size::Ratio(16.0 / 9.0)));
        assert_eq!(Size::parse("1536X1024"), Ok(Size::Pixels(1536, 1024)));
        assert!(Size::parse("big").is_err());
        assert!(Size::parse("0:1").is_err());
        assert!(Size::parse("10.5x3").is_err());

        let wide = Size::parse("16:9").unwrap();
        assert_eq!(wide.openai().as_deref(), Some("1536x1024"));
        assert_eq!(wide.ratio_among(&XAI_RATIOS), Some("16:9"));
        assert_eq!(
            wide.tier(&XAI_TIERS),
            None,
            "a ratio leaves the resolution to the provider"
        );
        assert_eq!(Size::Auto.openai(), None);

        let tall = Size::Pixels(900, 2000);
        assert_eq!(tall.openai().as_deref(), Some("900x2000"));
        assert_eq!(tall.ratio_among(&GOOGLE_RATIOS), Some("9:16"));
        assert_eq!(tall.tier(&GOOGLE_TIERS), Some("2K"));
        assert_eq!(Size::Pixels(5000, 5000).tier(&XAI_TIERS), Some("2k"));
    }
}
