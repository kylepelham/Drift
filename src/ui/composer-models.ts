import { formatModelContext, lmStudioModelReady } from "../state/lm-studio";
import { orderedModelProviderIds } from "../state/prefs";
import { smallContextTokens } from "../engine/store";
import { t } from "../state/i18n";

import type { EngineState } from "../engine/store";
import type { PickerItem } from "./picker";

const localProviders = ["ollama", "lmstudio"];

/** The picker's line under a model: a small or unknown window is warned about, and LM Studio shows its loaded window. */
export function modelDetail(providerID: string, model: { id: string; limit?: { context: number } }) {
    const context = model.limit?.context ?? 0;
    if (context > 0 && context < smallContextTokens)
        return t("drift.model.smallContext", { size: formatModelContext(context) });
    // A local model not yet loaded runs at whatever window its server picks, and compaction cannot plan for it.
    if (context === 0 && localProviders.includes(providerID)) return t("drift.model.unknownContext");
    return providerID === "lmstudio" ? `${model.id} | ${formatModelContext(context)} context` : undefined;
}

/** Every model of a connected provider, newest first within each provider. */
export function connectedModelItems(state: EngineState): PickerItem[] {
    const providers = state.providers.filter((provider) => {
        if (provider.id === "lmstudio") return state.connected.includes(provider.id);
        return state.connected.includes(provider.id) || (state.connection !== "online" && state.connected.length === 0);
    });
    const order = orderedModelProviderIds(providers.map((provider) => provider.id));
    return order.flatMap((providerID) => {
        const provider = providers.find((item) => item.id === providerID);
        if (!provider) return [];
        return Object.values(provider.models)
            .filter((model) => provider.id !== "lmstudio" || lmStudioModelReady(model))
            .sort((a, b) => (b.release_date ?? "").localeCompare(a.release_date ?? "") || a.name.localeCompare(b.name))
            .map((model) => ({
                id: `${provider.id}/${model.id}`,
                label: model.name,
                group: provider.name,
                detail: modelDetail(provider.id, model),
                providerID: provider.id,
                family: model.family,
                releaseDate: model.release_date,
            }));
    });
}
