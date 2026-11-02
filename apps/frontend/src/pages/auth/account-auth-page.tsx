import { submitAccountAuth } from "../../adapters/web/account-auth";
import { useAuth } from "@/context/auth-context";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  ApplicationShell,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Input,
  Label,
} from "@wealthfolio/ui";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { BrowserRouter, Link, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { z } from "zod";

type Mode = "login" | "signup" | "recover" | "reset";

function AccountForm({ mode }: { mode: Mode }) {
  const { t } = useTranslation();
  const { login, clearError, loginError } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const routeState: unknown = location.state;
  const notice =
    mode === "login" && routeState && typeof routeState === "object" && "notice" in routeState
      ? routeState.notice
      : undefined;
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const needsPassword = mode !== "recover";
  const schema = z.object({
    email: z.string().trim().email(t("auth:account.invalidEmail")).max(254),
    password: z
      .string()
      .max(1024)
      .refine(
        (v) => !needsPassword || (mode === "login" ? v.length > 0 : [...v].length >= 12),
        t("auth:account.passwordHelp"),
      ),
    invitationCode: z
      .string()
      .max(256)
      .refine((v) => mode !== "signup" || v.length > 0, t("auth:account.invitationRequired")),
    code: z
      .string()
      .max(128)
      .refine((v) => mode !== "reset" || v.length > 0, t("auth:account.codeRequired")),
  });
  const {
    register,
    handleSubmit,
    formState: { errors, isSubmitting },
    resetField,
  } = useForm<z.infer<typeof schema>>({
    resolver: zodResolver(schema),
    defaultValues: { email: "", password: "", invitationCode: "", code: "" },
  });
  const onSubmit = handleSubmit(async (values) => {
    setError("");
    setMessage("");
    clearError();
    try {
      if (mode === "login") {
        await login(values.password, values.email);
      } else {
        await submitAccountAuth(mode, values);
        resetField("password");
        resetField("code");
        resetField("invitationCode");
        if (mode === "recover") {
          setMessage(t("auth:account.recoverySent"));
        } else {
          navigate("/auth/login", { state: { notice: mode }, replace: true });
        }
      }
    } catch (cause) {
      setError(
        cause instanceof Error && cause.message === "rateLimited"
          ? t("auth:account.rateLimited")
          : mode === "login" && cause instanceof Error
            ? cause.message
            : t(`auth:account.${mode}Failed`),
      );
    }
  });
  return (
    <Card className="w-full">
      <CardHeader>
        <CardTitle>{t(`auth:account.${mode}Title`)}</CardTitle>
        <CardDescription>{t(`auth:account.${mode}Description`)}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-5">
        {(notice === "signup" || notice === "reset") && (
          <p role="status">
            {notice === "signup" ? t("auth:account.signupSuccess") : t("auth:account.resetSuccess")}
          </p>
        )}
        <form onSubmit={onSubmit} className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="account-email">{t("auth:account.email")}</Label>
            <Input
              id="account-email"
              type="email"
              autoComplete="username"
              maxLength={254}
              required
              disabled={isSubmitting}
              {...register("email")}
            />
            {errors.email && <p role="alert">{errors.email.message}</p>}
          </div>
          {mode === "signup" && (
            <div className="space-y-2">
              <Label htmlFor="invitation-code">{t("auth:account.invitation")}</Label>
              <Input
                id="invitation-code"
                autoComplete="off"
                maxLength={256}
                required
                disabled={isSubmitting}
                {...register("invitationCode")}
              />
              {errors.invitationCode && <p role="alert">{errors.invitationCode.message}</p>}
            </div>
          )}
          {mode === "reset" && (
            <div className="space-y-2">
              <Label htmlFor="recovery-code">{t("auth:account.code")}</Label>
              <Input
                id="recovery-code"
                autoComplete="one-time-code"
                maxLength={128}
                required
                disabled={isSubmitting}
                {...register("code")}
              />
              {errors.code && <p role="alert">{errors.code.message}</p>}
            </div>
          )}
          {needsPassword && (
            <div className="space-y-2">
              <Label htmlFor="account-password">{t("auth:account.password")}</Label>
              <Input
                id="account-password"
                type="password"
                autoComplete={mode === "login" ? "current-password" : "new-password"}
                maxLength={1024}
                required
                disabled={isSubmitting}
                {...register("password")}
              />
              {mode !== "login" && (
                <p className="text-muted-foreground text-sm">{t("auth:account.passwordHelp")}</p>
              )}
              {errors.password && <p role="alert">{errors.password.message}</p>}
            </div>
          )}
          {(error || (mode === "login" && loginError)) && (
            <p className="text-destructive text-sm" role="alert">
              {error || loginError}
            </p>
          )}
          {message && (
            <p role="status" className="text-sm">
              {message}
            </p>
          )}
          <Button type="submit" className="w-full" disabled={isSubmitting}>
            {isSubmitting ? t("auth:account.working") : t(`auth:account.${mode}Submit`)}
          </Button>
        </form>
        <nav className="flex flex-wrap gap-4 text-sm" aria-label={t("auth:account.navigation")}>
          {mode !== "login" && (
            <Link className="underline" to="/auth/login">
              {t("auth:account.loginTitle")}
            </Link>
          )}
          {mode === "login" && (
            <>
              <Link className="underline" to="/auth/signup">
                {t("auth:account.signupTitle")}
              </Link>
              <Link className="underline" to="/auth/recover">
                {t("auth:account.recoverTitle")}
              </Link>
            </>
          )}
          {mode === "recover" && (
            <Link className="underline" to="/auth/reset">
              {t("auth:account.haveCode")}
            </Link>
          )}
          {mode === "reset" && (
            <Link className="underline" to="/auth/recover">
              {t("auth:account.recoverTitle")}
            </Link>
          )}
        </nav>
      </CardContent>
    </Card>
  );
}

export function AccountAuthRoutes() {
  return (
    <ApplicationShell className="fixed inset-0 overflow-y-auto p-6">
      <div className="mx-auto flex min-h-full w-full max-w-md items-center py-6">
        <Routes>
          <Route path="/auth/signup" element={<AccountForm key="signup" mode="signup" />} />
          <Route path="/auth/recover" element={<AccountForm key="recover" mode="recover" />} />
          <Route path="/auth/reset" element={<AccountForm key="reset" mode="reset" />} />
          <Route path="*" element={<AccountForm key="login" mode="login" />} />
        </Routes>
      </div>
    </ApplicationShell>
  );
}

export function AccountAuthPage() {
  return (
    <BrowserRouter>
      <AccountAuthRoutes />
    </BrowserRouter>
  );
}
