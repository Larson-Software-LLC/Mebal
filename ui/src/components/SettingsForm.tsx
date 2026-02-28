import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Config, EncoderInfo } from "../types";

interface Props {
  config: Config;
  onSave: (config: Config) => Promise<boolean>;
}

const RESOLUTIONS: { label: string; value: [number, number] }[] = [
  { label: "1920x1080 (1080p)", value: [1920, 1080] },
  { label: "2560x1440 (1440p)", value: [2560, 1440] },
  { label: "3840x2160 (4K)", value: [3840, 2160] },
];

const FPS_OPTIONS = [30, 60, 120];

export default function SettingsForm({ config, onSave }: Props) {
  const [form, setForm] = useState<Config>(config);
  const [encoders, setEncoders] = useState<EncoderInfo[]>([]);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    invoke<EncoderInfo[]>("get_encoder_info").then(setEncoders);
  }, []);

  useEffect(() => {
    setForm(config);
    setDirty(false);
  }, [config]);

  const update = <K extends keyof Config>(key: K, value: Config[K]) => {
    setForm((prev) => ({ ...prev, [key]: value }));
    setDirty(true);
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setSaving(true);
    try {
      await onSave(form);
      setDirty(false);
    } finally {
      setSaving(false);
    }
  };

  const resKey = `${form.resolution[0]}x${form.resolution[1]}`;

  return (
    <form className="settings-form" onSubmit={handleSubmit}>
      <fieldset>
        <legend>Buffer</legend>
        <label>
          Buffer Duration (s)
          <input
            type="number"
            min={10}
            value={form.buffer_duration_secs}
            onChange={(e) =>
              update("buffer_duration_secs", Number(e.target.value))
            }
          />
        </label>
        <label>
          Save Duration (s)
          <input
            type="number"
            min={1}
            value={form.save_duration_secs}
            onChange={(e) =>
              update("save_duration_secs", Number(e.target.value))
            }
          />
        </label>
      </fieldset>

      <fieldset>
        <legend>Video</legend>
        <label>
          Resolution
          <select
            value={resKey}
            onChange={(e) => {
              const res = RESOLUTIONS.find(
                (r) => `${r.value[0]}x${r.value[1]}` === e.target.value,
              );
              if (res) update("resolution", res.value);
            }}
          >
            {RESOLUTIONS.map((r) => (
              <option
                key={`${r.value[0]}x${r.value[1]}`}
                value={`${r.value[0]}x${r.value[1]}`}
              >
                {r.label}
              </option>
            ))}
          </select>
        </label>
        <label>
          FPS
          <select
            value={form.fps}
            onChange={(e) => update("fps", Number(e.target.value))}
          >
            {FPS_OPTIONS.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
        </label>
        <label>
          Bitrate (kbps)
          <input
            type="number"
            min={1000}
            step={500}
            value={form.bitrate_kbps}
            onChange={(e) => update("bitrate_kbps", Number(e.target.value))}
          />
        </label>
        <label>
          Encoder
          <select
            value={form.encoder ?? ""}
            onChange={(e) =>
              update("encoder", e.target.value === "" ? null : e.target.value)
            }
          >
            <option value="">Auto-detect</option>
            {encoders.map((enc) => (
              <option key={enc.name} value={enc.name} disabled={!enc.available}>
                {enc.name} {enc.available ? "" : "(unavailable)"}
              </option>
            ))}
          </select>
        </label>
      </fieldset>

      <fieldset>
        <legend>Audio</legend>
        <label className="checkbox-label">
          <input
            type="checkbox"
            checked={form.audio_enabled}
            onChange={(e) => update("audio_enabled", e.target.checked)}
          />
          Enable Audio Capture
        </label>
        {form.audio_enabled && (
          <label>
            Audio Bitrate (kbps)
            <input
              type="number"
              min={64}
              step={32}
              value={form.audio_bitrate_kbps}
              onChange={(e) =>
                update("audio_bitrate_kbps", Number(e.target.value))
              }
            />
          </label>
        )}
      </fieldset>

      <fieldset>
        <legend>Output</legend>
        <label>
          Directory
          <input
            type="text"
            value={form.output_directory}
            onChange={(e) => update("output_directory", e.target.value)}
          />
        </label>
        <label>
          Filename Prefix
          <input
            type="text"
            value={form.output_prefix}
            onChange={(e) => update("output_prefix", e.target.value)}
          />
        </label>
      </fieldset>

      <fieldset>
        <legend>Hotkey</legend>
        <label>
          Hotkey
          <input
            type="text"
            value={form.hotkey}
            onChange={(e) => update("hotkey", e.target.value)}
            placeholder="e.g. F9, Ctrl+Shift+F9"
          />
        </label>
      </fieldset>

      <button type="submit" className="btn-save-settings" disabled={!dirty || saving}>
        {saving ? "Saving..." : "Save Settings"}
      </button>
    </form>
  );
}
