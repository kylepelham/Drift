/** Sign-in choices displayed by Settings, in the order supplied by engine actions. */
export type ProviderAuthMethod = { type: "oauth" | "api"; label: string };

export type AuthorizationPrompt = { code?: string; text?: string };

const deviceCode = /code:\s*([A-Z0-9][A-Z0-9-]{3,})\s*$/i;
const url = /https?:\/\/\S+/g;

// Plugins write free-form instructions; device codes get their own display and URLs sit behind buttons.
export function authorizationPrompt(instructions: string): AuthorizationPrompt {
    const code = instructions.match(deviceCode)?.[1];
    if (code) return { code };
    const text = instructions
        .replace(url, "")
        .replace(/[\s:]+$/, "")
        .trim();
    return text ? { text } : {};
}
