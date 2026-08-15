import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useI18n } from "./i18n";
import "./App.css";

type Phase = "idle" | "starting" | "ready" | "stopping" | "error";
type LogLine = { stream: string; line: string };
type StatusEvent = { phase: Phase; url: string | null; error: string | null };
type Status = StatusEvent & { logs: LogLine[] };

export default function App() {
  const { t } = useI18n();
  const [status, setStatus] = useState<StatusEvent | null>(null);
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [error, setError] = useState("");

  useEffect(() => {
    void invoke<Status>("dsh_status")
      .then(({ logs: lines, ...rest }) => { setStatus(rest); setLogs(lines); })
      .catch((reason) => setError(String(reason)));
    const off = listen<StatusEvent>("dsh-status", ({ payload }) => setStatus(payload));
    return () => { void off.then((dispose) => dispose()); };
  }, []);

  const failed = status?.phase === "error" || Boolean(error);

  // The broadcast carries no logs; fetch them for the failure report only.
  useEffect(() => {
    if (!failed) return;
    void invoke<Status>("dsh_status").then((current) => setLogs(current.logs)).catch(() => {});
  }, [failed]);

  return (
    <main className="boot">
      <span className={`boot-mark ${failed ? "failed" : ""}`} aria-hidden="true" />
      <h1>{failed ? t("boot.failed") : t("boot.starting")}</h1>
      <p>{t("boot.description")}</p>
      {failed ? <pre>{error || status?.error}</pre> : null}
      {failed && logs.length ? <details><summary>{t("boot.details")}</summary><pre>{logs.slice(-20).map((item) => item.line).join("\n")}</pre></details> : null}
    </main>
  );
}
