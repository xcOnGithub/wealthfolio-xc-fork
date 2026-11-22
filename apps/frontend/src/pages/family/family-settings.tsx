import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useFieldArray, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { Button, Input } from "@wealthfolio/ui";
import { Card, CardContent, CardHeader, CardTitle } from "@wealthfolio/ui/components/ui/card";
import type { FamilySnapshot, SaveFamilySettings } from "@/features/family/types";

const schema = z.object({
  enabled: z.boolean(),
  largeChargeAmount: z.number().finite().positive(),
  unusualMultiplier: z.number().finite().min(1.5).max(20),
  unusualMinimum: z.number().finite().positive(),
  budgetPercent: z.number().finite().min(10).max(200),
  consentNoticeDays: z.number().int().min(1).max(90),
  staleValuationDays: z.number().int().min(1).max(730),
  billNoticeDays: z.number().int().min(1).max(90),
  emailAlerts: z.boolean(),
  emailDigest: z.boolean(),
  webhookAlerts: z.boolean(),
  webhookDigest: z.boolean(),
  emailFrom: z.string().max(320),
  recipientsText: z.string().refine((s) =>
    s
      .split(",")
      .map((v) => v.trim())
      .filter(Boolean)
      .every((v) => z.email().safeParse(v).success),
  ),
  bills: z
    .array(
      z.object({
        id: z.string(),
        name: z.string().min(1).max(160),
        dueDate: z.string().regex(/^\d{4}-\d{2}-\d{2}$/),
        amount: z.number().finite().positive(),
        currency: z.string().regex(/^[A-Z]{3}$/),
        annual: z.boolean(),
      }),
    )
    .max(100),
});
type Values = z.infer<typeof schema>;
export function FamilySettingsPanel({
  snapshot,
  currency,
  onSave,
  busy,
}: {
  snapshot: FamilySnapshot;
  currency: string;
  onSave: (value: SaveFamilySettings) => Promise<unknown>;
  busy: boolean;
}) {
  const { t } = useTranslation("family");
  const { recipients, budgetThreshold, ...initial } = snapshot.settings;
  const {
    register,
    handleSubmit,
    control,
    formState: { errors },
  } = useForm<Values>({
    resolver: zodResolver(schema),
    defaultValues: {
      ...initial,
      recipientsText: recipients.join(", "),
      budgetPercent: budgetThreshold * 100,
    },
  });
  const { fields, append, remove } = useFieldArray({ control, name: "bills", keyName: "fieldKey" });
  const [resendApiKey, setKey] = useState("");
  const [webhookUrl, setUrl] = useState("");
  const [webhookToken, setToken] = useState("");
  const [result, setResult] = useState("");
  const submit = handleSubmit(async ({ recipientsText, budgetPercent, ...values }) => {
    setResult("");
    try {
      await onSave({
        settings: {
          ...values,
          budgetThreshold: budgetPercent / 100,
          recipients: recipientsText
            .split(",")
            .map((v) => v.trim())
            .filter(Boolean),
        },
        ...(resendApiKey ? { resendApiKey } : {}),
        ...(webhookUrl ? { webhookUrl } : {}),
        ...(webhookToken ? { webhookToken } : {}),
      });
      setKey("");
      setUrl("");
      setToken("");
      setResult(t("saved"));
    } catch {
      setResult(t("saveError"));
    }
  });
  const check = (
    name: "enabled" | "emailAlerts" | "emailDigest" | "webhookAlerts" | "webhookDigest",
    label: string,
  ) => (
    <label className="flex items-center gap-3 text-sm">
      <input type="checkbox" {...register(name)} />
      <span>{label}</span>
    </label>
  );
  return (
    <form onSubmit={submit} className="space-y-6" aria-label={t("settings")}>
      <Card>
        <CardHeader>
          <CardTitle>{t("rules")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-5">
          {check("enabled", t("enabled"))}
          <p className="text-muted-foreground text-sm">{t("thresholdCurrency", { currency })}</p>
          <div className="grid gap-4 md:grid-cols-2">
            {(
              [
                ["largeChargeAmount", "largeCharge"],
                ["unusualMultiplier", "unusualMultiplier"],
                ["unusualMinimum", "unusualMinimum"],
                ["budgetPercent", "budgetPercent"],
                ["consentNoticeDays", "consentDays"],
                ["staleValuationDays", "staleDays"],
                ["billNoticeDays", "billDays"],
              ] as const
            ).map(([field, label]) => (
              <label key={field} className="space-y-2 text-sm">
                <span>{t(label)}</span>
                <Input
                  type="number"
                  step={field === "unusualMultiplier" ? "0.5" : "1"}
                  {...register(field, { valueAsNumber: true })}
                  aria-invalid={!!errors[field]}
                />
              </label>
            ))}
          </div>
          <p className="text-muted-foreground text-sm">{t("unusualHelp")}</p>
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>{t("delivery")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-5">
          <p className="text-muted-foreground text-sm">{t("deliveryHelp")}</p>
          <div className="grid gap-4 md:grid-cols-2">
            {check("emailAlerts", t("emailAlerts"))}
            {check("emailDigest", t("emailDigest"))}
            {check("webhookAlerts", t("webhookAlerts"))}
            {check("webhookDigest", t("webhookDigest"))}
          </div>
          <label className="block space-y-2 text-sm">
            <span>{t("sender")}</span>
            <Input {...register("emailFrom")} />
          </label>
          <label className="block space-y-2 text-sm">
            <span>{t("recipients")}</span>
            <Input {...register("recipientsText")} aria-invalid={!!errors.recipientsText} />
          </label>
          <label className="block space-y-2 text-sm">
            <span>{t("emailKey")}</span>
            <Input
              type="password"
              autoComplete="new-password"
              value={resendApiKey}
              onChange={(e) => setKey(e.target.value)}
            />
            <span className="text-muted-foreground">
              {snapshot.resendConfigured ? t("configured") : t("notConfigured")}
            </span>
          </label>
          <label className="block space-y-2 text-sm">
            <span>{t("webhookUrl")}</span>
            <Input
              type="password"
              autoComplete="new-password"
              value={webhookUrl}
              onChange={(e) => setUrl(e.target.value)}
            />
            <span className="text-muted-foreground">
              {snapshot.webhookConfigured ? t("configured") : t("notConfigured")}
            </span>
          </label>
          <label className="block space-y-2 text-sm">
            <span>{t("webhookToken")}</span>
            <Input
              type="password"
              autoComplete="new-password"
              value={webhookToken}
              onChange={(e) => setToken(e.target.value)}
            />
          </label>
          <p className="text-muted-foreground text-sm">{t("credentialsHelp")}</p>
          <p className="text-muted-foreground text-sm">{t("scheduleHelp")}</p>
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>{t("bills")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <p className="text-muted-foreground text-sm">{t("billsHelp")}</p>
          {fields.map((field, index) => (
            <div key={field.fieldKey} className="space-y-3 rounded-lg border p-4">
              <div className="grid gap-3 md:grid-cols-2">
                <label className="space-y-1 text-sm">
                  <span>{t("billName")}</span>
                  <Input {...register(`bills.${index}.name`)} />
                </label>
                <label className="space-y-1 text-sm">
                  <span>{t("dueDate")}</span>
                  <Input type="date" {...register(`bills.${index}.dueDate`)} />
                </label>
                <label className="space-y-1 text-sm">
                  <span>{t("amount")}</span>
                  <Input
                    type="number"
                    step="0.01"
                    {...register(`bills.${index}.amount`, { valueAsNumber: true })}
                  />
                </label>
                <label className="space-y-1 text-sm">
                  <span>{t("currency")}</span>
                  <Input maxLength={3} {...register(`bills.${index}.currency`)} />
                </label>
              </div>
              <div className="flex flex-wrap items-center justify-between gap-3">
                <label className="flex items-center gap-2 text-sm">
                  <input type="checkbox" {...register(`bills.${index}.annual`)} />
                  {t("annual")}
                </label>
                <Button type="button" variant="ghost" onClick={() => remove(index)}>
                  {t("removeBill")}
                </Button>
              </div>
            </div>
          ))}
          <Button
            type="button"
            variant="outline"
            onClick={() =>
              append({
                id: crypto.randomUUID(),
                name: "",
                dueDate: new Date().toISOString().slice(0, 10),
                amount: 0,
                currency,
                annual: true,
              })
            }
          >
            {t("addBill")}
          </Button>
        </CardContent>
      </Card>
      {Object.keys(errors).length > 0 && (
        <p role="alert" className="text-destructive text-sm">
          {t("invalidSettings")}
        </p>
      )}
      {result && (
        <p role="status" className="text-sm">
          {result}
        </p>
      )}
      <Button type="submit" disabled={busy}>
        {busy ? t("saving") : t("save")}
      </Button>
    </form>
  );
}
