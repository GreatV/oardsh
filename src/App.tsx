import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./App.css";

type Tool = {
  path: string;
  version: string | null;
};

type Environment = {
  node: Tool | null;
  npx: Tool | null;
  dsh: Tool | null;
};

type Phase = "idle" | "starting" | "ready" | "stopping" | "error";

type LogLine = {
  stream: string;
  line: string;
};

type Status = {
  phase: Phase;
  url: string | null;
  workspace: string | null;
  command: string | null;
  error: string | null;
  attached: boolean;
  logs: LogLine[];
};

const DEFAULT_STATUS: Status = {
  phase: "starting",
  url: null,
  workspace: null,
  command: null,
  error: null,
  attached: false,
  logs: [],
};

export default function App() {
  const [environment, setEnvironment] = useState<Environment | null>(null);
  const [status, setStatus] = useState<Status>(DEFAULT_STATUS);
  const [logs, setLogs] = useState<LogLine[]>([]);
  const logRef = useRef<HTMLPreElement>(null);
  const missingRuntime =
    environment !== null && (!environment.node || !environment.dsh);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([invoke<Environment>("probe_environment"), invoke<Status>("get_status")])
      .then(([env, current]) => {
        if (cancelled) return;
        setEnvironment(env);
        setStatus(current);
        setLogs(current.logs ?? []);
      })
      .catch((err) => {
        if (!cancelled) {
          setStatus((prev) => ({
            ...prev,
            phase: "error",
            error: String(err),
          }));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const unlistenStatus = listen<Status>("dsh-status", (event) => {
      setStatus(event.payload);
      if (event.payload.logs?.length) {
        setLogs(event.payload.logs);
      }
    });
    const unlistenLog = listen<LogLine>("dsh-log", (event) => {
      setLogs((prev) => [...prev.slice(-240), event.payload]);
    });
    return () => {
      void unlistenStatus.then((fn) => fn());
      void unlistenLog.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const node = logRef.current;
    if (node) node.scrollTop = node.scrollHeight;
  }, [logs]);

  const failed = status.phase === "error" || missingRuntime;

  return (
    <main className={`boot ${failed ? "failed" : ""}`}>
      <div className="mark" aria-hidden="true">
        <span />
        <span />
      </div>
      <h1>DeepSeek Harness</h1>
      <p className="status">
        {missingRuntime
          ? "Node.js or the bundled @deepseek-ai/dsh package is missing. Run npm install and try again."
          : status.phase === "error"
            ? "dsh web failed to start."
            : status.phase === "ready"
              ? "Opening…"
              : "Starting dsh web…"}
      </p>
      {status.workspace ? <p className="workspace">{status.workspace}</p> : null}
      {status.error ? <pre className="error">{status.error}</pre> : null}
      {missingRuntime ? (
        <button type="button" onClick={() => void openUrl("https://nodejs.org")}>
          Get Node.js
        </button>
      ) : null}
      {status.phase === "error" ? (
        <button type="button" onClick={() => void invoke("retry_dsh")}>
          Retry
        </button>
      ) : null}
      <pre ref={logRef} className="log">
        {logs.map((entry, index) => (
          <span key={`${index}-${entry.line}`} className={entry.stream}>
            {entry.line}
            {"\n"}
          </span>
        ))}
      </pre>
    </main>
  );
}
