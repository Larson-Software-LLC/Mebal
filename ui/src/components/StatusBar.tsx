import type { BufferStatus } from "../types";

interface Props {
  status: BufferStatus | null;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export default function StatusBar({ status }: Props) {
  if (!status) {
    return (
      <div className="status-bar">
        <span className="indicator stopped" />
        <span>Waiting for status...</span>
      </div>
    );
  }

  return (
    <div className="status-bar">
      <span
        className={`indicator ${status.isCapturing ? "recording" : "stopped"}`}
      />
      <span>{status.isCapturing ? "Recording" : "Stopped"}</span>
      <span className="sep">|</span>
      <span>{status.durationSecs}s buffered</span>
      <span className="sep">|</span>
      <span>
        {formatBytes(status.totalBytes)} / {formatBytes(status.maxBytes)}
      </span>
      {status.isSaving && <span className="saving-badge">Saving...</span>}
      <div className="progress-bar">
        <div
          className="progress-fill"
          style={{ width: `${Math.min(status.utilizationPercent, 100)}%` }}
        />
      </div>
    </div>
  );
}
