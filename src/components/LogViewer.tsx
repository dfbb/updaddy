import { useEffect, useRef } from "react";
import type { LogEntry } from "../types";

interface LogViewerProps {
  entries: LogEntry[];
  emptyLabel?: string;
  consoleMode?: boolean;
  follow?: boolean;
}

function formatTimestamp(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "—";
  return new Date(value * 1000).toLocaleString();
}

export default function LogViewer({ entries, emptyLabel = "No logs", consoleMode = false, follow = false }: LogViewerProps) {
  const viewer = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (follow && viewer.current) viewer.current.scrollTop = viewer.current.scrollHeight;
  }, [entries, follow]);
  if (entries.length === 0) return <p className="log-empty">{emptyLabel}</p>;
  return (
    <div ref={viewer} className={`log-viewer ${consoleMode ? "log-console" : ""}`} role="log" aria-live="polite">
      {entries.map((entry, index) => (
        <div className="log-entry" key={`${entry.emitted_at}-${index}`}>
          {!consoleMode && <time dateTime={new Date(entry.emitted_at * 1000).toISOString()}>{formatTimestamp(entry.emitted_at)}</time>}
          {!consoleMode && <span className="log-stream">{entry.stream}</span>}
          <pre>{entry.message}</pre>
        </div>
      ))}
    </div>
  );
}

export { formatTimestamp };
