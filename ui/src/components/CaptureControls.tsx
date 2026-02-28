import { invoke } from "@tauri-apps/api/core";
import type { BufferStatus } from "../types";

interface Props {
  status: BufferStatus | null;
}

export default function CaptureControls({ status }: Props) {
  const capturing = status?.isCapturing ?? false;
  const saving = status?.isSaving ?? false;

  const handleToggleCapture = async () => {
    if (capturing) {
      await invoke("stop_capture");
    } else {
      await invoke("start_capture");
    }
  };

  const handleSave = async () => {
    await invoke("save_replay");
  };

  return (
    <div className="capture-controls">
      <button
        onClick={handleToggleCapture}
        className={capturing ? "btn-stop" : "btn-start"}
      >
        {capturing ? "Stop Capture" : "Start Capture"}
      </button>
      <button
        onClick={handleSave}
        disabled={!capturing || saving}
        className="btn-save"
      >
        {saving ? "Saving..." : "Save Replay"}
      </button>
    </div>
  );
}
