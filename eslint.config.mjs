import eslint from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: ["node_modules/", "target/", "outlook-addin/.generated/"],
  },
  eslint.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  {
    files: ["outlook-addin/**/*.ts"],
    languageOptions: {
      parserOptions: {
        project: "./outlook-addin/tsconfig.json",
        tsconfigRootDir: import.meta.dirname,
      },
    },
  },
  {
    files: ["tools/**/*.mjs", "eslint.config.mjs"],
    ...tseslint.configs.disableTypeChecked,
  },
);
