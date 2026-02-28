import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { BufferStatus } from "../types";

export function useBufferStatus(): BufferStatus | null {
  const [status, setStatus] = useState<BufferStatus | null>(null);

  useEffect(() => {
    const unlisten = listen<BufferStatus>("buffer-status", (event) => {
      setStatus(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return status;
}
