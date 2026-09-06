import { readFile, mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const schemaPath = resolve(root, "contracts/ipc/skeleton-status.schema.json");
const outputPath = resolve(root, "outlook-addin/.generated/skeleton-status.ts");
const schema = JSON.parse(await readFile(schemaPath, "utf8"));

const exactKeys = [
  "contract_version",
  "companion_state",
  "enabled_capabilities",
  "gates_passed",
];
function hasExactKeys(value, expected) {
  return (
    JSON.stringify(Object.keys(value).sort()) ===
    JSON.stringify([...expected].sort())
  );
}
if (
  schema.additionalProperties !== false ||
  !hasExactKeys(schema, [
    "$schema",
    "$id",
    "title",
    "type",
    "additionalProperties",
    "required",
    "properties",
  ]) ||
  schema.type !== "object" ||
  JSON.stringify(schema.required) !== JSON.stringify(exactKeys) ||
  !hasExactKeys(schema.properties, exactKeys) ||
  !hasExactKeys(schema.properties.contract_version, ["const"]) ||
  schema.properties.contract_version.const !== 1 ||
  !hasExactKeys(schema.properties.companion_state, ["enum"]) ||
  JSON.stringify(schema.properties.companion_state.enum) !==
    JSON.stringify(["skeleton_disabled"]) ||
  !hasExactKeys(schema.properties.enabled_capabilities, ["type", "maxItems"]) ||
  schema.properties.enabled_capabilities.type !== "array" ||
  schema.properties.enabled_capabilities.maxItems !== 0 ||
  !hasExactKeys(schema.properties.gates_passed, ["type", "maxItems"]) ||
  schema.properties.gates_passed.type !== "array" ||
  schema.properties.gates_passed.maxItems !== 0
) {
  throw new Error("Closed synthetic skeleton contract changed");
}

const generated = `// Generated from the public content-free schema; do not track this file.\n\
export interface SkeletonStatus {\n\
  readonly contract_version: 1;\n\
  readonly companion_state: "skeleton_disabled";\n\
  readonly enabled_capabilities: readonly [];\n\
  readonly gates_passed: readonly [];\n\
}\n`;

await mkdir(resolve(outputPath, ".."), { recursive: true });
await writeFile(outputPath, generated, { encoding: "utf8", flag: "w" });
