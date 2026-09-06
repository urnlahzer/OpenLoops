import type { SkeletonStatus } from "../.generated/skeleton-status.js";

export type OutlookItemType = Office.MailboxEnums.ItemType;

export function syntheticStatus(): SkeletonStatus {
  return {
    contract_version: 1,
    companion_state: "skeleton_disabled",
    enabled_capabilities: [],
    gates_passed: [],
  };
}
