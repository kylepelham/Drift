import perfectionist from "eslint-plugin-perfectionist";
import tseslint from "typescript-eslint";
import solid from "eslint-plugin-solid";

// Formatting is Prettier's; these are the rules a formatter cannot check: Solid reactivity, import order and the
// style guide's limits on complexity and density.
export default tseslint.config(
    {
        ignores: [
            ".build/",
            ".worktrees/",
            "dist/",
            "target/",
            "examples/",
            "src-tauri/",
            "plugins/",
            "src/engine/native/types.ts",
        ],
    },
    {
        files: ["src/**/*.{ts,tsx}", "tests/**/*.ts", "scripts/**/*.ts", "vite.config.ts", "eslint.config.ts"],
        extends: [tseslint.configs.base],
        plugins: { perfectionist },
        rules: {
            complexity: ["error", 15],
            "max-statements-per-line": ["error", { max: 1 }],
            "no-duplicate-imports": ["error", { allowSeparateTypeImports: true }],
            "no-nested-ternary": "error",
            "@typescript-eslint/consistent-type-imports": ["error", { fixStyle: "separate-type-imports" }],
            "perfectionist/sort-imports": [
                "error",
                {
                    type: "line-length",
                    order: "desc",
                    customGroups: [
                        { groupName: "value-singleline", selector: "import", modifiers: ["value", "singleline"] },
                        { groupName: "value-multiline", selector: "import", modifiers: ["value", "multiline"] },
                        { groupName: "type-singleline", selector: "import", modifiers: ["type", "singleline"] },
                        { groupName: "type-multiline", selector: "import", modifiers: ["type", "multiline"] },
                    ],
                    groups: [
                        "side-effect",
                        "value-singleline",
                        "value-multiline",
                        { newlinesBetween: 1 },
                        "type-singleline",
                        "type-multiline",
                        "unknown",
                    ],
                    newlinesBetween: 0,
                },
            ],
        },
    },
    {
        files: ["src/**/*.tsx"],
        ...solid.configs["flat/typescript"],
    },
);
