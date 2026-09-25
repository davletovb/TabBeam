export default [
  {
    files: ["src/**/*.js"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: {
        chrome: "readonly",
        console: "readonly",
        crypto: "readonly",
        document: "readonly",
        window: "readonly",
        URL: "readonly",
        URLSearchParams: "readonly",
        HTMLElement: "readonly",
        HTMLButtonElement: "readonly",
        HTMLFormElement: "readonly",
        HTMLTextAreaElement: "readonly",
        TextEncoder: "readonly",
        NodeFilter: "readonly"
      }
    },
    rules: {
      "no-undef": "error",
      "no-unused-vars": ["error", { "argsIgnorePattern": "^_" }],
      "no-unreachable": "error",
      "no-constant-condition": "error"
    }
  },
  {
    files: ["scripts/**/*.mjs", "tests/**/*.mjs", "eslint.config.js"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: {
        console: "readonly",
        globalThis: "readonly",
        process: "readonly",
        TextEncoder: "readonly",
        TextDecoder: "readonly",
        setTimeout: "readonly",
        URL: "readonly"
      }
    },
    rules: {
      "no-undef": "error",
      "no-unused-vars": ["error", { "argsIgnorePattern": "^_" }],
      "no-unreachable": "error",
      "no-constant-condition": "error"
    }
  }
];
