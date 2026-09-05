import type { LogEntry } from "../types";

interface LogViewerProps {
  entries: LogEntry[];
  emptyLabel?: string;
}

function formatTimestamp(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "—";
  return new Date(value * 1000).toLocaleString();
}

export default function LogViewer({ entries, emptyLabel = "No logs" }: LogViewerProps) {
  if (entries.length === 0) return <p className="log-empty">{emptyLabel}</p>;
  return (
    <div className="log-viewer" role="log" aria-live="polite">
      {entries.map((entry, index) => (
        <div className="log-entry" key={`${entry.emitted_at}-${index}`}>
          <time dateTime={new Date(entry.emitted_at * 1000).toISOString()}>{formatTimestamp(entry.emitted_at)}</time>
          <span className="log-stream">{entry.stream}</span>
          <pre>{entry.message}</pre>
        </div>
      ))}
    </div>
  );
}

export { formatTimestamp };
