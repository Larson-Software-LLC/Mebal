import { useBufferStatus } from "./hooks/useBufferStatus";
import { useConfig } from "./hooks/useConfig";
import StatusBar from "./components/StatusBar";
import CaptureControls from "./components/CaptureControls";
import SettingsForm from "./components/SettingsForm";

function App() {
  const status = useBufferStatus();
  const { config, updateConfig, loading, error } = useConfig();

  if (loading) {
    return <div className="app loading">Loading...</div>;
  }

  if (error) {
    return (
      <div className="app loading">
        <div>
          <p>Failed to load configuration</p>
          <pre style={{ color: "#e05050", fontSize: 12, whiteSpace: "pre-wrap" }}>{error}</pre>
        </div>
      </div>
    );
  }

  if (!config) {
    return <div className="app loading">No configuration found</div>;
  }

  return (
    <div className="app">
      <StatusBar status={status} />
      <CaptureControls status={status} />
      <SettingsForm config={config} onSave={updateConfig} />
    </div>
  );
}

export default App;
