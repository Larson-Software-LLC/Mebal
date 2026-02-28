import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Config } from "../types";

export function useConfig() {
  const [config, setConfig] = useState<Config | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<Config>("get_config")
      .then((cfg) => {
        setConfig(cfg);
        setLoading(false);
      })
      .catch((err) => {
        console.error("Failed to load config:", err);
        setError(String(err));
        setLoading(false);
      });
  }, []);

  const updateConfig = async (newConfig: Config): Promise<boolean> => {
    const needsRestart = await invoke<boolean>("set_config", {
      config: newConfig,
    });
    setConfig(newConfig);
    return needsRestart;
  };

  return { config, updateConfig, loading, error };
}
