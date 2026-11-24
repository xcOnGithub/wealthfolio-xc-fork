import { invoke } from "./platform";
import type {
  FamilySnapshot,
  SaveFamilySettings,
  FamilyItemAction,
  FamilyBulkItemAction,
  FamilyTask,
  MonthlyFamilyDigest,
} from "@/features/family/types";
export const getFamilySnapshot = () => invoke<FamilySnapshot>("get_family_snapshot");
export const refreshFamilyInbox = () => invoke<FamilySnapshot>("refresh_family_inbox");
export const saveFamilySettings = (input: SaveFamilySettings) =>
  invoke<FamilySnapshot>("save_family_settings", { input });
export const updateFamilyItem = (input: FamilyItemAction) =>
  invoke<FamilySnapshot>("update_family_item", { input });
export const updateFamilyItems = (input: FamilyBulkItemAction) =>
  invoke<FamilySnapshot>("update_family_items", { input });
export const saveFamilyTask = (input: FamilyTask) =>
  invoke<FamilySnapshot>("save_family_task", { input });
export const previewFamilyDigest = (month: string) =>
  invoke<MonthlyFamilyDigest>("preview_family_digest", { month });
export const retryFamilyDelivery = (id: string) =>
  invoke<FamilySnapshot>("retry_family_delivery", { id });
