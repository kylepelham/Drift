//! The size a page lays out at: the browser's own, a tablet's, or a phone's with touch and a mobile user agent.

use std::sync::Arc;

use serde_json::{Value, json};

use super::instance::Instance;
use super::page::Page;

struct Size {
    width: u64,
    height: u64,
    mobile: bool,
}

fn size(input: &Value) -> Result<Option<Size>, String> {
    let preset = input["preset"].as_str();
    let custom = input["width"].as_u64().zip(input["height"].as_u64());

    match (preset, custom) {
        (Some("desktop"), _) => Ok(None),
        (Some("tablet"), _) => Ok(Some(Size {
            width: 768,
            height: 1024,
            mobile: false,
        })),
        (Some("mobile"), _) => Ok(Some(Size {
            width: 390,
            height: 844,
            mobile: true,
        })),
        (Some(other), _) => Err(format!("`{other}` is not a preset; use desktop, tablet or mobile")),
        (None, Some((width, height))) => Ok(Some(Size {
            width,
            height,
            mobile: width < 768,
        })),
        (None, None) => Err("`preset`, or `width` and `height`, is required".into()),
    }
}

/// Lays the page out at the size asked for; a phone's size also brings touch and a phone's user agent.
pub(crate) async fn set(instance: &Arc<Instance>, page: &Page, input: &Value) -> Result<String, String> {
    let wanted = size(input)?;
    let call = |method: &'static str, params: Value| async move {
        page.call(method, params).await.map_err(|error| error.to_string())
    };

    let Some(size) = wanted else {
        call("Emulation.clearDeviceMetricsOverride", json!({})).await?;
        call("Emulation.setTouchEmulationEnabled", json!({ "enabled": false })).await?;
        let desktop = instance
            .agents
            .desktop
            .clone()
            .unwrap_or_else(|| json!({ "userAgent": "" }));
        call("Emulation.setUserAgentOverride", desktop).await?;
        page.reload().await?;
        return Ok("The page is back at the browser's own size and reloaded.".into());
    };

    let scale = if size.mobile { 2 } else { 1 };
    let metrics = json!({
        "width": size.width,
        "height": size.height,
        "deviceScaleFactor": scale,
        "mobile": size.mobile,
    });
    call("Emulation.setDeviceMetricsOverride", metrics).await?;
    call(
        "Emulation.setTouchEmulationEnabled",
        json!({ "enabled": size.mobile, "maxTouchPoints": 5 }),
    )
    .await?;
    if size.mobile {
        call("Emulation.setUserAgentOverride", instance.agents.mobile.clone()).await?;
    }
    // Pages decide on a phone layout or user agent as they load.
    page.reload().await?;

    let kind = if size.mobile { " as a phone, with touch," } else { "" };
    Ok(format!(
        "The page is laid out at {}x{}{kind} and was reloaded.",
        size.width, size.height
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_sizes() {
        assert!(size(&json!({ "preset": "desktop" })).unwrap().is_none());
        let phone = size(&json!({ "preset": "mobile" })).unwrap().unwrap();
        assert!(phone.mobile);
        let narrow = size(&json!({ "width": 500, "height": 900 })).unwrap().unwrap();
        assert!(narrow.mobile, "under 768 wide is a phone");
        assert!(size(&json!({ "preset": "watch" })).is_err());
        assert!(size(&json!({})).is_err());
    }
}
