import { useId, useRef } from "react";
import { useTranslation } from "react-i18next";
import { Button, Icons, Label } from "@wealthfolio/ui";

interface StatementFileInputProps {
  disabled: boolean;
  onSelect: (files: File[]) => void;
}

export function StatementFileInput({ disabled, onSelect }: StatementFileInputProps) {
  const { t } = useTranslation();
  const inputId = useId();
  const inputRef = useRef<HTMLInputElement>(null);

  return (
    <div className="grid gap-2">
      <Label htmlFor={inputId}>{t("account:statements.upload")}</Label>
      <div>
        <input
          ref={inputRef}
          id={inputId}
          type="file"
          accept=".pdf,application/pdf"
          multiple
          tabIndex={-1}
          disabled={disabled}
          className="sr-only"
          onChange={(e) => {
            const files = Array.from(e.target.files ?? []);
            e.target.value = "";
            if (files.length) onSelect(files);
          }}
        />
        <Button
          type="button"
          variant="outline"
          disabled={disabled}
          onClick={() => inputRef.current?.click()}
        >
          <Icons.Upload aria-hidden />
          {t("ui:dataGrid.selectFilesToUpload")}
        </Button>
      </div>
    </div>
  );
}
