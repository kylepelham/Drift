//! The account's offers applied to the catalog: speed modes it cannot use go, Daybreak entries come.

use std::collections::BTreeMap;

use serde_json::json;

use super::Offer;
use crate::llm::catalog::{Model, ModelMode, ProviderInfo};

const DAYBREAK: &str = "daybreak";

/// Removes modes whose `service_tier` the account's list does not offer for their model; Ultrafast from
/// models.dev describes the API, and the Codex backend quietly serves it at standard speed. Models the
/// list does not name are left as they are.
pub(super) fn drop_unoffered_speeds(provider: &mut ProviderInfo, offers: &BTreeMap<String, Offer>) {
    provider.models.retain(|_, model| {
        let Some(mode) = &model.mode else {
            return true;
        };
        let Some(tier) = mode.body.get("service_tier").and_then(|tier| tier.as_str()) else {
            return true;
        };

        offers
            .get(&mode.base)
            .is_none_or(|offer| offer.tiers.iter().any(|offered| offered == tier))
    });
}

/// Adds `<id>-daybreak`, named `<Name> Daybreak`, for every entry whose model the account offers a program for.
pub(super) fn add_daybreak(provider: &mut ProviderInfo, offers: &BTreeMap<String, Offer>) {
    let additions: Vec<_> = provider
        .models
        .values()
        .filter(|model| model.mode.as_ref().is_none_or(|mode| mode.name != DAYBREAK))
        .filter_map(|model| {
            let program = offers.get(model.wire(&model.id))?.program.as_deref()?;
            Some(daybreak(model, program))
        })
        .collect();

    provider
        .models
        .extend(additions.into_iter().map(|model| (model.id.clone(), model)));
}

/// The same model, any speed mode kept, with the program selected in the request body.
fn daybreak(base: &Model, program: &str) -> Model {
    let wire = base.wire(&base.id).to_string();
    let mut mode = base.mode.clone().unwrap_or_else(|| ModelMode {
        name: String::new(),
        base: wire,
        body: Default::default(),
        headers: Default::default(),
    });
    mode.name = DAYBREAK.into();
    mode.body.insert("access_programs".into(), json!({ "cyber": program }));

    Model {
        id: format!("{}-{DAYBREAK}", base.id),
        name: format!("{} Daybreak", base.name),
        mode: Some(mode),
        ..base.clone()
    }
}
