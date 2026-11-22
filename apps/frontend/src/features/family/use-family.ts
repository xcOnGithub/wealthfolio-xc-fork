import {
  getFamilySnapshot,
  refreshFamilyInbox,
  saveFamilySettings,
  updateFamilyItem,
  retryFamilyDelivery,
} from "@/adapters";
import { selectedProfileId } from "@/features/profiles/session";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { FamilySnapshot } from "./types";

export function useFamily() {
  const profile = selectedProfileId();
  const key = ["family-money", profile];
  const client = useQueryClient();
  const query = useQuery({ queryKey: key, queryFn: getFamilySnapshot, refetchInterval: 60_000 });
  const saved = (snapshot: FamilySnapshot) => {
    if (profile === selectedProfileId()) client.setQueryData(key, snapshot);
  };
  const refresh = useMutation({ mutationFn: refreshFamilyInbox, onSuccess: saved });
  const settings = useMutation({ mutationFn: saveFamilySettings, onSuccess: saved });
  const action = useMutation({ mutationFn: updateFamilyItem, onSuccess: saved });
  const retry = useMutation({ mutationFn: retryFamilyDelivery, onSuccess: saved });
  return { ...query, refresh, settings, action, retry };
}
