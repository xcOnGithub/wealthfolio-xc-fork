import { Icons } from "@wealthfolio/ui";
import { useTranslation } from "react-i18next";
import { useFamily } from "./use-family";
export function FamilyInboxIcon() {
  const { t } = useTranslation("family");
  const { data } = useFamily();
  const count = data?.items.filter((item) => item.active && item.status === "open").length ?? 0;
  return (
    <span className="relative inline-flex">
      <Icons.Mail className="size-6" />
      {count > 0 && (
        <span
          aria-label={t("unread", { count })}
          className="bg-primary text-primary-foreground text-2xs absolute -end-1 -top-1 flex min-h-4 min-w-4 items-center justify-center rounded-full px-1"
        >
          {count > 9 ? "9+" : count}
        </span>
      )}
    </span>
  );
}
